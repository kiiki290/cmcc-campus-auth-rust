//! 认证核心：探测 Portal → 启动 Chromium → 识别验证码 → JS 提交 → 确认放行。
//!
//! 与原 Python 版保持相同的关键行为：
//! - 认证表单必须由页面内 JS `form.submit()` 提交（瑞数动态防护会拦截纯 HTTP POST）；
//! - 优先按 URL 含 `unionautologin.do` 定位认证 frame，找不到时回落 `frames[1]`；
//! - 页面提示成功 ≠ BRAS 已放行，需用 1.1.1.1 递增等待确认。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::anyhow;
use chromiumoxide::Page;
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use serde::Deserialize;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use url::Url;

use crate::browser::{self, BrowserSession};
use crate::config::AppConfig;
use crate::events::{EventSink, Phase};
use crate::ocr::{self, OcrEngine};
use crate::probe::{self, PortalProbe};

pub const PORTAL: &str = "http://218.200.239.185:8888/portalserver";
pub const MAX_RETRY: usize = 3;
pub const RETRY_DELAY: Duration = Duration::from_secs(5);
pub const POLL_TIMEOUT: Duration = Duration::from_secs(10);
pub const RESULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Portal 根地址：优先环境变量（测试/换 Portal 用），否则从 302 URL 推导，
/// 推导失败再回落到内置常量。
fn portal_base(outer_url: &str) -> String {
    if let Ok(base) = std::env::var("CMCC_PORTAL_BASE") {
        let base = base.trim().trim_end_matches('/');
        if !base.is_empty() {
            return base.to_string();
        }
    }
    const SUFFIX: &str = "/portalserver";
    if let Some(index) = outer_url.find(SUFFIX) {
        return outer_url[..index + SUFFIX.len()].to_string();
    }
    PORTAL.to_string()
}

const FATAL_PATTERNS: &[&str] = &[
    "密码错误",
    "密码不正确",
    "密码无效",
    "密码错",
    "账号不存在",
    "用户不存在",
    "帐号不存在",
    "欠费",
    "已停机",
    "已停用",
    "暂停使用",
    "已锁定",
    "已冻结",
    "已禁用",
    "已注销",
    "账号过期",
    "帐号过期",
    "已到期",
];

const DEFAULT_PORTAL_HOST: &str = "218.200.239.185";

/// 只允许把密码提交给已知 Portal 主机（以及本机回环，便于 mock 测试）。
/// 其他部署可通过 `CMCC_PORTAL_HOST=a.b.c.d,portal.example` 追加白名单。
fn validate_url_host(url: &Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("认证页协议不受支持：{}", url.scheme()));
    }
    let Some(host) = url.host_str() else {
        return Err("认证页地址缺少主机名".to_string());
    };
    let matches_host = |candidate: &str| candidate.eq_ignore_ascii_case(host);

    let allow_loopback =
        cfg!(test) || cfg!(debug_assertions) || std::env::var_os("CMCC_ALLOW_LOOPBACK").is_some();
    if allow_loopback
        && (matches_host("127.0.0.1") || matches_host("localhost") || matches_host("::1"))
    {
        return Ok(());
    }

    // 已知 Portal 必须匹配主机和端口（原 Portal 为 http://218.200.239.185:8888）
    if matches_host(DEFAULT_PORTAL_HOST) && url.port() == Some(8888) {
        return Ok(());
    }

    if let Ok(extra) = std::env::var("CMCC_PORTAL_HOST") {
        for entry in extra
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            let (entry_host, entry_port) = match entry.rsplit_once(':') {
                Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, p.parse::<u16>().ok()),
                _ => (entry, None),
            };
            if entry_host.eq_ignore_ascii_case(host)
                && (entry_port.is_none() || entry_port == url.port())
            {
                return Ok(());
            }
        }
    }

    let shown_port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Err(format!(
        "认证页主机 {host}{shown_port} 不在允许列表；为防止密码泄露已中止。\
         如确认这是你的 Portal，请设置环境变量 CMCC_PORTAL_HOST={host}{shown_port}"
    ))
}

fn validate_portal_url(outer_url: &str) -> Result<Url, String> {
    let parsed = Url::parse(outer_url).map_err(|err| format!("认证页地址无效：{err}"))?;
    validate_url_host(&parsed)?;
    Ok(parsed)
}

/// 校验表单实际提交目标：必须与认证 frame 同源，且通过主机白名单。
fn validate_submit_target(action: &str, base: &Url) -> Result<(), String> {
    if action.trim().is_empty() {
        return Ok(());
    }
    let target = base
        .join(action)
        .map_err(|err| format!("表单 action 无效：{err}"))?;
    validate_url_host(&target)?;
    if target.origin() != base.origin() {
        return Err(format!(
            "表单提交目标与认证页不同源（{} → {}），已中止",
            base.origin().ascii_serialization(),
            target.origin().ascii_serialization()
        ));
    }
    Ok(())
}
#[derive(Clone)]
pub struct Credentials {
    pub student_id: String,
    pub password: String,
}

impl Credentials {
    pub fn masked_student_id(&self) -> String {
        let chars: Vec<char> = self.student_id.chars().collect();
        if chars.len() <= 4 {
            return "*".repeat(chars.len());
        }
        let visible = 3;
        let mut masked: String = chars[..visible].iter().collect();
        masked.push_str(&"*".repeat(chars.len() - visible));
        masked
    }
}

/// 取消标记：停止监控时置位，认证流程在关键节点检查。
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub enum AuthOutcome {
    AlreadyOnline,
    Authenticated,
}

#[derive(Debug, Clone)]
pub enum AuthError {
    Fatal(String),
    Failed(String),
    Network(String),
    Cancelled,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Fatal(msg) => write!(f, "{msg}"),
            AuthError::Failed(msg) => write!(f, "{msg}"),
            AuthError::Network(msg) => write!(f, "{msg}"),
            AuthError::Cancelled => write!(f, "已取消"),
        }
    }
}

#[derive(Debug)]
pub enum RetryOutcome {
    Online,
    Authenticated,
    Fatal(String),
    Failed(String),
    Cancelled,
}

/// 完整认证（含 3 轮重试和 5 秒间隔）。
pub async fn authenticate_with_retry(
    creds: &Credentials,
    cfg: &AppConfig,
    sink: &EventSink,
    cancel: &CancelFlag,
) -> RetryOutcome {
    let mut last_error = String::new();
    for attempt in 1..=MAX_RETRY {
        if cancel.is_cancelled() {
            return RetryOutcome::Cancelled;
        }
        if attempt > 1 {
            sink.info(format!("第 {attempt}/{MAX_RETRY} 次尝试 ..."));
        }
        match authenticate(creds, cfg, sink, cancel).await {
            Ok(AuthOutcome::AlreadyOnline) => return RetryOutcome::Online,
            Ok(AuthOutcome::Authenticated) => return RetryOutcome::Authenticated,
            Err(AuthError::Cancelled) => return RetryOutcome::Cancelled,
            Err(AuthError::Fatal(msg)) => {
                sink.error(format!("✗ 致命错误，不再重试：{msg}"));
                return RetryOutcome::Fatal(msg);
            }
            Err(AuthError::Failed(msg)) | Err(AuthError::Network(msg)) => {
                sink.error(format!("认证失败：{msg}"));
                last_error = msg;
            }
        }
        if attempt < MAX_RETRY {
            sink.info("5 秒后重试 ...");
            if !sleep_cancellable(RETRY_DELAY, cancel).await {
                return RetryOutcome::Cancelled;
            }
        }
    }
    RetryOutcome::Failed(last_error)
}

async fn authenticate(
    creds: &Credentials,
    cfg: &AppConfig,
    sink: &EventSink,
    cancel: &CancelFlag,
) -> Result<AuthOutcome, AuthError> {
    if cancel.is_cancelled() {
        return Err(AuthError::Cancelled);
    }

    // 1. 探测 Portal
    sink.info("获取 Portal 地址 ...");
    sink.send(crate::events::EngineEvent::Phase(Phase::Probing));
    let probe = tokio::task::spawn_blocking(probe::probe_portal)
        .await
        .map_err(|err| AuthError::Network(format!("探测任务失败：{err}")))?;

    let outer_url = match probe {
        PortalProbe::Portal(url) => url,
        PortalProbe::Online => {
            sink.success("✓ 已在线，无需认证");
            return Ok(AuthOutcome::AlreadyOnline);
        }
        PortalProbe::Error(err) if probe::is_quiet_online_error(&err) => {
            sink.success("✓ 已在线，无需认证");
            return Ok(AuthOutcome::AlreadyOnline);
        }
        PortalProbe::Error(err) => {
            sink.error(format!("✗ 网络不可达：{err}"));
            return Err(AuthError::Network(format!("网络不可达：{err}")));
        }
    };

    if let Err(err) = validate_portal_url(&outer_url) {
        sink.error(format!("✗ {err}"));
        return Err(AuthError::Fatal(err));
    }

    if let Some(ip) = probe::extract_wan_ip(&outer_url) {
        sink.info(format!("  WAN IP = {ip}"));
        sink.send(crate::events::EngineEvent::WanIp(Some(ip)));
    } else {
        let preview: String = outer_url.chars().take(60).collect();
        sink.info(format!("  Portal = {preview}..."));
    }

    if cancel.is_cancelled() {
        return Err(AuthError::Cancelled);
    }

    sink.send(crate::events::EngineEvent::Phase(Phase::Authenticating));

    // 2. OCR 模型在后台加载，与浏览器启动并行
    let ocr_task = tokio::task::spawn_blocking(ocr::load_cached);

    // 3. 启动浏览器
    let resolved = browser::resolve(cfg).map_err(AuthError::Failed)?;
    sink.send(crate::events::EngineEvent::BrowserResolved(
        resolved.description(),
    ));
    let session = browser::launch(&resolved, sink)
        .await
        .map_err(|err| AuthError::Failed(format!("{err:#}")))?;

    let result = if cancel.is_cancelled() {
        Err(AuthError::Cancelled)
    } else {
        run_flow(&session, &outer_url, creds, sink, cancel, ocr_task).await
    };
    session.shutdown().await;
    result
}

async fn run_flow(
    session: &BrowserSession,
    outer_url: &str,
    creds: &Credentials,
    sink: &EventSink,
    cancel: &CancelFlag,
    ocr_task: JoinHandle<anyhow::Result<Arc<OcrEngine>>>,
) -> Result<AuthOutcome, AuthError> {
    let page = &session.page;

    // 加载认证页
    sink.info("加载认证页面 ...");
    match tokio::time::timeout(Duration::from_secs(15), page.goto(outer_url)).await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            return Err(AuthError::Failed(format!("加载认证页面失败：{err}")));
        }
        Err(_) => {
            return Err(AuthError::Failed("加载认证页面超时（15 秒）".to_string()));
        }
    }

    // 定位认证 frame（优先 unionautologin.do，回落 frames[1]）
    let info = wait_for_auth_doc(page, cancel).await?;
    sink.success("  认证页面加载成功");
    let frame_url = match validate_portal_url(&info.url) {
        Ok(url) => url,
        Err(err) => {
            sink.error(format!("✗ {err}"));
            return Err(AuthError::Fatal(err));
        }
    };
    if let Err(err) = validate_submit_target(&info.action, &frame_url) {
        sink.error(format!("✗ {err}"));
        return Err(AuthError::Fatal(err));
    }
    let mut jsessionid = jsessionid_from_action(&info.action).unwrap_or_default();
    if jsessionid.is_empty() {
        jsessionid = jsessionid_from_cookies(page).await.unwrap_or_default();
    }
    if jsessionid.is_empty() {
        jsessionid = jsessionid_from_action(&info.url).unwrap_or_default();
    }
    if jsessionid.is_empty() {
        sink.warn("  ⚠ 未获取到 JSESSIONID，验证码可能无法正常获取");
    } else {
        let short: String = jsessionid.chars().take(12).collect();
        sink.info(format!("  JSESSIONID = {short}..."));
    }

    // 等待 OCR 模型就绪
    let ocr = match ocr_task.await {
        Ok(Ok(engine)) => engine,
        Ok(Err(err)) => {
            sink.error(format!("  ✗ OCR 初始化失败：{err:#}"));
            return Err(AuthError::Failed(format!("OCR 初始化失败：{err:#}")));
        }
        Err(err) => {
            sink.error(format!("  ✗ OCR 任务异常：{err}"));
            return Err(AuthError::Failed(format!("OCR 任务异常：{err}")));
        }
    };

    if cancel.is_cancelled() {
        return Err(AuthError::Cancelled);
    }

    // 4. 获取验证码并识别（最多 3 次）
    let portal_base = portal_base(outer_url);
    sink.info("识别验证码 ...");
    let mut captcha = String::new();
    for attempt in 1..=3 {
        let cap_url = format!(
            "{portal_base}/user/randomimage{}?id={}",
            if jsessionid.is_empty() {
                String::new()
            } else {
                format!(";jsessionid={jsessionid}")
            },
            rand::random_range(100_000..=999_999)
        );

        let fetch_script = CAPTCHA_FETCH_TEMPLATE
            .replace("__FINDER__", FIND_AUTH_DOC)
            .replace("__URL__", &js_string(&cap_url));
        let response = match eval_json_timeout(page, &fetch_script, Duration::from_secs(12)).await {
            Ok(value) => value,
            Err(err) => {
                sink.warn(format!("  第 {attempt} 次验证码获取失败：{err}"));
                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let payload: CaptchaFetch = match serde_json::from_value(response) {
            Ok(payload) => payload,
            Err(err) => {
                sink.warn(format!("  第 {attempt} 次验证码响应异常：{err}"));
                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        if !payload.ok || !payload.content_type.to_ascii_lowercase().contains("image") {
            sink.warn(format!(
                "  第 {attempt} 次验证码获取失败（HTTP {}，{}）",
                payload.status, payload.content_type
            ));
            sleep(Duration::from_secs(1)).await;
            continue;
        }
        let Some(data) = payload.data else {
            sink.warn(format!("  第 {attempt} 次验证码响应没有图片数据"));
            continue;
        };
        let bytes = match base64_decode(&data) {
            Ok(bytes) => bytes,
            Err(err) => {
                sink.warn(format!("  第 {attempt} 次验证码数据解码失败：{err}"));
                continue;
            }
        };

        let ocr = ocr.clone();
        let recognized = tokio::task::spawn_blocking(move || ocr.recognize(&bytes))
            .await
            .map_err(|err| AuthError::Failed(format!("OCR 任务异常：{err}")))?;
        match recognized {
            Ok(code) => {
                sink.info(format!("  第 {attempt} 次识别：{code}"));
                if code.chars().count() >= 4 {
                    captcha = code;
                    break;
                }
            }
            Err(err) => {
                sink.warn(format!("  第 {attempt} 次识别失败：{err:#}"));
            }
        }
    }

    if captcha.is_empty() {
        return Err(AuthError::Failed("验证码识别失败".to_string()));
    }

    if cancel.is_cancelled() {
        return Err(AuthError::Cancelled);
    }

    // 5. 填表并由页面 JS 提交
    sink.info("提交登录表单 ...");
    let submit_script = SUBMIT_TEMPLATE
        .replace("__FINDER__", FIND_AUTH_DOC)
        .replace("__SID__", &js_string(&creds.student_id))
        .replace("__PWD__", &js_string(&creds.password))
        .replace("__CAPTCHA__", &js_string(&captcha))
        .replace("__JSESSIONID__", &js_string(&jsessionid));

    match eval_json_timeout(page, &submit_script, Duration::from_secs(10)).await {
        Ok(value) => {
            if value.get("ok").and_then(Value::as_bool) != Some(true) {
                let reason = value
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("未知原因");
                return Err(AuthError::Failed(format!("提交登录表单失败：{reason}")));
            }
        }
        Err(err) => {
            // 表单提交通常立即触发导航，求值可能因执行上下文销毁而报错
            sink.warn(format!("  ⚠ 提交后页面导航（忽略求值异常）：{err}"));
        }
    }

    // 6. 轮询结果页
    let result = wait_for_result(page, cancel).await?;
    match classify_result(&result.title, &result.admin, &result.text) {
        ResultKind::Success(admin) => {
            if admin.is_empty() {
                sink.success("✓ 认证成功！");
            } else {
                sink.success(format!("✓ 认证成功！{admin}"));
            }
        }
        ResultKind::Fatal(message) => {
            sink.error(format!("✗ 登录失败 — {message}"));
            return Err(AuthError::Fatal(message));
        }
        ResultKind::Failed(message) => {
            sink.error(format!("✗ {message}"));
            return Err(AuthError::Failed(message));
        }
    }

    // 7. 确认 BRAS 真正放行（0/1/2/3/5/8s 递增，总窗口约 19s）
    for (index, delay) in [0u64, 1, 2, 3, 5, 8].into_iter().enumerate() {
        if delay > 0 && !sleep_cancellable(Duration::from_secs(delay), cancel).await {
            return Err(AuthError::Cancelled);
        }
        let state = tokio::task::spawn_blocking(probe::is_released)
            .await
            .map_err(|err| AuthError::Failed(format!("放行确认任务失败：{err}")))?;
        match state {
            Some(true) => {
                sink.success("✓ 已确认在线（BRAS 放行）");
                return Ok(AuthOutcome::Authenticated);
            }
            Some(false) if index == 0 => {
                sink.info("  等待 BRAS 放行 ...");
            }
            Some(false) => {}
            None => {
                sink.warn("  ⚠ 确认探测异常（状态未知），本轮按失败处理，稍后整轮重试");
                return Err(AuthError::Failed("确认探测异常（状态未知）".to_string()));
            }
        }
    }

    sink.error("✗ 页面已接受但 BRAS 未放行，按失败重试");
    Err(AuthError::Failed("BRAS 未放行".to_string()))
}

// ── 页面内 JS ────────────────────────────────────────────────────────────────

/// 定位认证文档：优先 URL 含 unionautologin.do 的 frame，其次任意含表单的 frame，
/// 最后回落 frames[1]（Portal 调整 frameset 结构时索引可能错位）。
const FIND_AUTH_DOC: &str = r#"(() => {
  const hasForm = d => !!(d && d.querySelector('form') && d.querySelector('#username'));
  const frames = Array.from(document.querySelectorAll('frame, iframe'));
  try { if (hasForm(document)) return document; } catch (e) {}
  const byUrl = frames.find(f => ((f.getAttribute('src') || f.src || '').includes('unionautologin.do')));
  if (byUrl) { try { if (hasForm(byUrl.contentDocument)) return byUrl.contentDocument; } catch (e) {} }
  for (const f of frames) { try { if (hasForm(f.contentDocument)) return f.contentDocument; } catch (e) {} }
  const second = frames[1];
  if (second) { try { return second.contentDocument; } catch (e) {} }
  return null;
})()"#;

const AUTH_DOC_INFO_TEMPLATE: &str = r#"(() => {
  const doc = __FINDER__;
  if (!doc) return {found:false};
  const html = doc.documentElement ? doc.documentElement.outerHTML : '';
  const form = doc.querySelector('form');
  return {
    found: true,
    hasForm: !!form,
    hasUser: !!doc.querySelector('#username'),
    length: html.length,
    action: form ? String(form.action) : '',
    url: doc.location ? String(doc.location.href) : ''
  };
})()"#;

const CAPTCHA_FETCH_TEMPLATE: &str = r#"(() => {
  const doc = __FINDER__;
  const view = (doc && doc.defaultView) ? doc.defaultView : window;
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 8000);
  return view.fetch(__URL__, {credentials: 'include', cache: 'no-store', signal: ctl.signal})
    .then(async r => {
      clearTimeout(timer);
      const type = r.headers.get('content-type') || '';
      if (!r.ok) return {ok:false, status:r.status, contentType:type, data:null};
      const b = await r.arrayBuffer();
      const bytes = new Uint8Array(b);
      let binary = '';
      const chunk = 0x8000;
      for (let i = 0; i < bytes.length; i += chunk) {
        binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
      }
      return {ok:true, status:r.status, contentType:type, data:btoa(binary)};
    })
    .catch(e => { clearTimeout(timer); return {ok:false, status:0, contentType:'', data:null}; });
})()"#;

const RESULT_TEMPLATE: &str = r#"(() => {
  const docs = [];
  try { docs.push(document); } catch (e) {}
  for (const f of document.querySelectorAll('frame, iframe')) {
    try { if (f.contentDocument) docs.push(f.contentDocument); } catch (e) {}
  }
  for (const d of docs) {
    const title = (d.title || '').trim();
    const adminEl = d.querySelector('#admin');
    const admin = adminEl ? ((adminEl.innerText || adminEl.textContent || '').trim()) : '';
    const html = d.documentElement ? d.documentElement.outerHTML : '';
    if (title === '登录成功' || title === '登录失败' || admin || html.includes('登录成功') || html.includes('登录失败')) {
      return {title, admin, text: html.slice(0, 200000), url: d.location ? String(d.location.href) : ''};
    }
  }
  return null;
})()"#;

const SUBMIT_TEMPLATE: &str = r#"(() => {
  const doc = __FINDER__;
  if (!doc) return {ok:false, reason:'找不到认证 Frame'};
  const f = doc.querySelector('form');
  if (!f) return {ok:false, reason:'找不到登录表单'};
  const set = (sel, val) => {
    const el = doc.querySelector(sel);
    if (!el) return false;
    el.focus();
    el.value = val;
    el.dispatchEvent(new Event('input', {bubbles:true}));
    el.dispatchEvent(new Event('change', {bubbles:true}));
    el.blur();
    return true;
  };
  if (!set('#username', __SID__)) return {ok:false, reason:'用户名字段缺失'};
  if (!set('#pwd', __PWD__)) return {ok:false, reason:'密码字段缺失'};
  if (!set('#ps', __CAPTCHA__)) return {ok:false, reason:'验证码字段缺失'};
  const jsid = __JSESSIONID__;
  if (jsid && !String(f.action).includes('jsessionid')) {
    f.action = String(f.action).replace(
      'unionautologin.do',
      'unionautologin.do;jsessionid=' + jsid
    );
  }
  // 必须由页面 JS 触发 form.submit()，否则 WAF 返回 202 挑战页
  setTimeout(() => { try { f.submit(); } catch (e) {} }, 0);
  return {ok:true, action:String(f.action)};
})()"#;

#[derive(Debug, Deserialize, Default)]
struct AuthDocInfo {
    #[serde(default)]
    found: bool,
    #[serde(default, rename = "hasForm")]
    has_form: bool,
    #[serde(default, rename = "hasUser")]
    has_user: bool,
    #[serde(default)]
    length: usize,
    #[serde(default)]
    action: String,
    #[serde(default)]
    url: String,
}

#[derive(Debug, Deserialize, Default)]
struct CaptchaFetch {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    status: u16,
    #[serde(default, rename = "contentType")]
    content_type: String,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ResultInfo {
    #[serde(default)]
    title: String,
    #[serde(default)]
    admin: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ResultKind {
    Success(String),
    Fatal(String),
    Failed(String),
}

fn classify_result(title: &str, admin: &str, text: &str) -> ResultKind {
    let admin = compress_whitespace(admin);

    // 标题成功时以标题为准，避免成功页里的隐藏文案/JS 字符串触发致命错误误判
    if title.contains("登录成功") {
        return ResultKind::Success(admin);
    }

    // 可见的 #admin 文本是明确失败信息：优先匹配致命错误
    if let Some(pattern) = FATAL_PATTERNS
        .iter()
        .find(|pattern| admin.contains(*pattern))
    {
        return ResultKind::Fatal(if admin.is_empty() {
            (*pattern).to_string()
        } else {
            format!("{pattern} — {admin}")
        });
    }

    if title.contains("登录失败") {
        // 失败页的错误信息不一定在 #admin 中（可能只在正文/脚本里），
        // 标题已明确失败，此时检查整页不会再误伤成功页。
        if let Some(pattern) = FATAL_PATTERNS
            .iter()
            .find(|pattern| text.contains(*pattern))
        {
            return ResultKind::Fatal((*pattern).to_string());
        }
        return ResultKind::Failed(if admin.is_empty() {
            "登录失败".to_string()
        } else {
            format!("登录失败 — {admin}")
        });
    }

    let success_marker =
        text.contains("logout") || text.contains("online") || text.contains("登录成功");
    if success_marker {
        return ResultKind::Success(admin);
    }

    // 没有标题/成功标记时，再检查整页 HTML 中的致命错误与失败提示
    if let Some(pattern) = FATAL_PATTERNS
        .iter()
        .find(|pattern| text.contains(*pattern))
    {
        return ResultKind::Fatal((*pattern).to_string());
    }

    if text.contains("登录失败") {
        return ResultKind::Failed(if admin.is_empty() {
            "登录失败".to_string()
        } else {
            format!("登录失败 — {admin}")
        });
    }

    let preview: String = text
        .chars()
        .take(200)
        .collect::<String>()
        .replace('\n', " ");
    ResultKind::Failed(format!("服务器返回异常响应：{preview}"))
}
async fn eval_json(page: &Page, script: &str) -> anyhow::Result<Value> {
    let params = EvaluateParams::builder()
        .expression(script)
        .await_promise(true)
        .return_by_value(true)
        .build()
        .map_err(|err| anyhow!("构造 JS 求值参数失败：{err}"))?;
    let result = page.evaluate_expression(params).await?;
    Ok(result.value().cloned().unwrap_or(Value::Null))
}

async fn eval_json_timeout(page: &Page, script: &str, timeout: Duration) -> anyhow::Result<Value> {
    match tokio::time::timeout(timeout, eval_json(page, script)).await {
        Ok(result) => result,
        Err(_) => Err(anyhow!("JS 求值超时（{} 秒）", timeout.as_secs())),
    }
}

async fn wait_for_auth_doc(page: &Page, cancel: &CancelFlag) -> Result<AuthDocInfo, AuthError> {
    let script = AUTH_DOC_INFO_TEMPLATE.replace("__FINDER__", FIND_AUTH_DOC);
    let start = std::time::Instant::now();
    let mut last: Option<AuthDocInfo> = None;
    loop {
        if cancel.is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        if start.elapsed() > POLL_TIMEOUT {
            if let Some(info) = last.filter(|i| i.found && i.has_form) {
                return Ok(info);
            }
            return Err(AuthError::Failed(
                "找不到认证 Frame，请检查网络连接或稍后重试".to_string(),
            ));
        }
        match eval_json_timeout(page, &script, Duration::from_secs(5)).await {
            Ok(value) => {
                if let Ok(info) = serde_json::from_value::<AuthDocInfo>(value) {
                    if info.found && info.has_form && (info.has_user || info.length > 200) {
                        return Ok(info);
                    }
                    last = Some(info);
                }
            }
            Err(_) => {
                // 页面仍在加载 / frame 切换中，继续等待
            }
        }
        sleep(Duration::from_millis(300)).await;
    }
}

async fn wait_for_result(page: &Page, cancel: &CancelFlag) -> Result<ResultInfo, AuthError> {
    let start = std::time::Instant::now();
    while start.elapsed() < RESULT_TIMEOUT {
        if cancel.is_cancelled() {
            return Err(AuthError::Cancelled);
        }
        match eval_json_timeout(page, RESULT_TEMPLATE, Duration::from_secs(5)).await {
            Ok(Value::Null) => {}
            Ok(value) => {
                if let Ok(info) = serde_json::from_value::<ResultInfo>(value)
                    && (!info.title.is_empty()
                        || !info.admin.is_empty()
                        || info.text.contains("登录成功")
                        || info.text.contains("登录失败"))
                {
                    return Ok(info);
                }
            }
            Err(_) => {}
        }
        sleep(Duration::from_millis(300)).await;
    }
    Err(AuthError::Failed(format!(
        "等待认证结果超时（{} 秒）",
        RESULT_TIMEOUT.as_secs()
    )))
}

async fn jsessionid_from_cookies(page: &Page) -> Option<String> {
    let cookies = page.get_cookies().await.ok()?;
    cookies
        .into_iter()
        .find(|cookie| cookie.name == "JSESSIONID" && !cookie.value.is_empty())
        .map(|cookie| cookie.value)
}

fn jsessionid_from_action(action: &str) -> Option<String> {
    let (_, rest) = action.split_once("jsessionid=")?;
    let id: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    if id.is_empty() { None } else { Some(id) }
}

fn compress_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn base64_decode(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(data)
}

pub async fn sleep_cancellable(duration: Duration, cancel: &CancelFlag) -> bool {
    let deadline = std::time::Instant::now() + duration;
    while std::time::Instant::now() < deadline {
        if cancel.is_cancelled() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        sleep(remaining.min(Duration::from_millis(200))).await;
    }
    !cancel.is_cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsessionid() {
        let action = "/portalserver/user/unionautologin.do;jsessionid=51B9DC8F82AB5CFC08241F5DEEA59A70?brasip=1";
        assert_eq!(
            jsessionid_from_action(action).as_deref(),
            Some("51B9DC8F82AB5CFC08241F5DEEA59A70")
        );
        assert_eq!(jsessionid_from_action("unionautologin.do?x=1"), None);
        assert_eq!(
            jsessionid_from_action("unionautologin.do;jsessionid=ZZZ"),
            None
        );
    }

    #[test]
    fn derives_portal_base() {
        assert_eq!(
            portal_base("http://218.200.239.185:8888/portalserver/login.jsp?wlanuserip=1.2.3.4"),
            "http://218.200.239.185:8888/portalserver"
        );
        assert_eq!(
            portal_base("http://127.0.0.1:18888/portalserver/user/unionautologin.do"),
            "http://127.0.0.1:18888/portalserver"
        );
        assert_eq!(portal_base("http://example.com/"), PORTAL);
    }

    #[test]
    fn compresses_whitespace() {
        assert_eq!(compress_whitespace("  a\n b\t c "), "a b c");
    }

    #[test]
    fn classifies_fatal_even_without_exact_title() {
        assert!(matches!(
            classify_result("登录失败！", "密码错误，请重试", ""),
            ResultKind::Fatal(_)
        ));
        assert!(matches!(
            classify_result("", "", "<html>账户已欠费</html>"),
            ResultKind::Fatal(_)
        ));
    }

    #[test]
    fn failed_title_with_fatal_text_is_fatal() {
        assert!(matches!(
            classify_result("登录失败", "", "<html>密码错误</html>"),
            ResultKind::Fatal(_)
        ));
        assert!(matches!(
            classify_result("登录失败！", "", "<html>账户已欠费</html>"),
            ResultKind::Fatal(_)
        ));
    }

    #[test]
    fn success_page_with_hidden_fatal_text_is_still_success() {
        assert!(matches!(
            classify_result(
                "登录成功",
                "认证成功",
                "<script>var msg = '密码错误'; // 暂停使用</script>"
            ),
            ResultKind::Success(_)
        ));
    }

    #[test]
    fn classifies_success_and_failure() {
        assert!(matches!(
            classify_result("登录成功", "欢迎", ""),
            ResultKind::Success(_)
        ));
        assert!(matches!(
            classify_result("", "认证成功", "<html>logout</html>"),
            ResultKind::Success(_)
        ));
        assert!(matches!(
            classify_result("登录失败", "验证码错误", ""),
            ResultKind::Failed(_)
        ));
    }

    #[test]
    fn validates_portal_hosts() {
        assert!(validate_portal_url("http://218.200.239.185:8888/portalserver/x").is_ok());
        assert!(validate_portal_url("http://218.200.239.185/portalserver/x").is_err());
        assert!(validate_portal_url("http://127.0.0.1:18888/portalserver/x").is_ok());
        assert!(validate_portal_url("ftp://218.200.239.185:8888/x").is_err());
        assert!(validate_portal_url("http://evil.example/login").is_err());
    }

    #[test]
    fn validates_submit_target() {
        let base =
            Url::parse("http://127.0.0.1:18888/portalserver/user/unionautologin.do").unwrap();
        assert!(
            validate_submit_target("/portalserver/user/unionautologin.do;jsessionid=ABC", &base)
                .is_ok()
        );
        assert!(validate_submit_target("http://evil.example/steal", &base).is_err());

        let portal = Url::parse("http://218.200.239.185:8888/portalserver/user/x").unwrap();
        assert!(validate_submit_target("https://218.200.239.185:8888/same", &portal).is_err());
    }

    #[test]
    fn js_escape() {
        assert_eq!(js_string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    // ── 端到端测试：本地 mock Portal + 真实 Chromium ────────────────────────
    //
    // 运行方式（需先准备 chrome-headless-shell）：
    //   CMCC_BUNDLED_CHROMIUM=/path/to/chrome-headless-shell \
    //   CMCC_ALLOW_LOOPBACK=1 cargo test mock_portal_auth_flow -- --ignored --nocapture
    mod mock {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;
        use std::time::Duration;

        pub struct MockServer {
            pub port: u16,
            stop: Arc<AtomicBool>,
            join: Option<thread::JoinHandle<()>>,
        }

        impl MockServer {
            pub fn start_portal() -> Self {
                Self::start(true)
            }

            pub fn start_release() -> Self {
                Self::start(false)
            }

            fn start(portal: bool) -> Self {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let port = listener.local_addr().unwrap().port();
                listener.set_nonblocking(true).unwrap();
                let stop = Arc::new(AtomicBool::new(false));
                let stop_thread = stop.clone();
                let join = thread::spawn(move || {
                    while !stop_thread.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                thread::spawn(move || {
                                    if portal {
                                        handle_portal(stream, port);
                                    } else {
                                        handle_release(stream);
                                    }
                                });
                            }
                            Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(10));
                            }
                            Err(_) => break,
                        }
                    }
                });
                Self {
                    port,
                    stop,
                    join: Some(join),
                }
            }
        }

        impl Drop for MockServer {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Relaxed);
                let _ = TcpStream::connect(("127.0.0.1", self.port));
                if let Some(join) = self.join.take() {
                    let _ = join.join();
                }
            }
        }

        fn read_request(stream: &mut TcpStream) -> (String, String, Vec<u8>) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                let n = match stream.read(&mut chunk) {
                    Ok(n) if n > 0 => n,
                    _ => return (String::new(), String::new(), Vec::new()),
                };
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos + 4;
                }
                if buf.len() > 64 * 1024 {
                    return (String::new(), String::new(), Vec::new());
                }
            };
            let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let mut lines = head.split("\r\n");
            let request_line = lines.next().unwrap_or_default();
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or_default().to_string();
            let content_length = lines
                .filter_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .next()
                .unwrap_or(0);
            let mut body = buf[header_end..].to_vec();
            while body.len() < content_length {
                match stream.read(&mut chunk) {
                    Ok(n) if n > 0 => body.extend_from_slice(&chunk[..n]),
                    _ => break,
                }
            }
            (method, path, body)
        }

        fn respond(
            stream: &mut TcpStream,
            status: &str,
            content_type: &str,
            extra_headers: &str,
            body: &[u8],
        ) {
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }

        const CAPTCHA_PNG: &[u8] = include_bytes!("../assets/test/captcha_6971.png");
        const JSESSIONID: &str = "51B9DC8F82AB5CFC08241F5DEEA59A70";

        fn handle_portal(mut stream: TcpStream, port: u16) {
            let (method, path, body) = read_request(&mut stream);
            if method == "GET" && path.starts_with("/portalserver/user/randomimage") {
                respond(&mut stream, "200 OK", "image/png", "", CAPTCHA_PNG);
            } else if method == "GET" && path.starts_with("/portalserver/user/unionautologin.do") {
                let html = format!(
                    "<html><head><title>登录</title></head><body>\
                     <form id=\"inputForm\" action=\"/portalserver/user/unionautologin.do;jsessionid={JSESSIONID}?brasip=1\" method=\"post\">\
                     <input type=\"text\" id=\"username\" name=\"name\">\
                     <input type=\"text\" id=\"passwordIn_1\" name=\"password\" value=\"请输入上网密码\">\
                     <input type=\"password\" id=\"pwd\" name=\"pass\">\
                     <input type=\"text\" id=\"ps\" name=\"psNum\">\
                     </form></body></html>"
                );
                let cookie = format!("Set-Cookie: JSESSIONID={JSESSIONID}; Path=/portalserver\r\n");
                respond(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=UTF-8",
                    &cookie,
                    html.as_bytes(),
                );
            } else if method == "POST" && path.starts_with("/portalserver/user/unionautologin.do") {
                println!("mock portal POST body: {}", String::from_utf8_lossy(&body));
                respond(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=UTF-8",
                    "",
                    "<html><head><title>登录成功</title></head><body><div id=\"admin\">认证成功</div></body></html>".as_bytes(),
                );
            } else if method == "GET" && path.starts_with("/portalserver/login.jsp") {
                let html = "<html><head><title>portal</title></head><frameset rows=\"*\">\
                            <frame src=\"/portalserver/user/unionautologin.do?brasip=1&area=union&wlanuserip=10.9.8.7&domain=@cmccgxsd\" \
                            name=\"main\" id=\"topFrame1\"></frameset></html>";
                respond(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=UTF-8",
                    "",
                    html.as_bytes(),
                );
            } else if method == "GET" && path == "/" {
                let location = format!(
                    "http://127.0.0.1:{port}/portalserver/login.jsp?wlanuserip=10.9.8.7&wlanacname="
                );
                let extra = format!("Location: {location}\r\n");
                respond(&mut stream, "302 Found", "text/plain", &extra, b"");
            } else {
                respond(&mut stream, "404 Not Found", "text/plain", "", b"not found");
            }
        }

        fn handle_release(mut stream: TcpStream) {
            let _ = read_request(&mut stream);
            respond(&mut stream, "200 OK", "text/plain", "", b"ok");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "需要 CMCC_BUNDLED_CHROMIUM 指向 chrome-headless-shell"]
    async fn mock_portal_auth_flow() {
        use crate::config::{AppConfig, BrowserMode};
        use crate::events::{EngineEvent, EventSink};

        let portal = mock::MockServer::start_portal();
        let release = mock::MockServer::start_release();

        // 仅测试进程使用；测试需单独运行（--test-threads=1）
        unsafe {
            std::env::set_var("CMCC_PROBE_TARGET", format!("127.0.0.1:{}", portal.port));
            std::env::set_var("CMCC_RELEASE_TARGET", format!("127.0.0.1:{}", release.port));
            std::env::set_var("CMCC_NO_SANDBOX", "1");
            std::env::set_var("CMCC_ALLOW_LOOPBACK", "1");
        }

        let cfg = AppConfig {
            browser_mode: BrowserMode::Auto,
            ..Default::default()
        };
        let creds = Credentials {
            student_id: "test123".to_string(),
            password: "secret".to_string(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let sink = EventSink::new(tx);
        let cancel = CancelFlag::new();

        let outcome = tokio::time::timeout(
            Duration::from_secs(120),
            authenticate_with_retry(&creds, &cfg, &sink, &cancel),
        )
        .await
        .expect("认证超时");

        while let Ok(event) = rx.try_recv() {
            if let EngineEvent::Log(line) = event {
                println!("[{}] {}", line.time, line.text);
            }
        }

        assert!(
            matches!(outcome, RetryOutcome::Authenticated),
            "认证结果：{outcome:?}"
        );
    }
}
