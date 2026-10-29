//! Chromium 定位与启动。
//!
//! 认证必须由真实 Chromium 内核执行 JS（瑞数动态防护会拦截纯 HTTP POST），
//! 因此程序支持两种来源：
//! - 随包 Chromium/chrome-headless-shell：放在程序目录 `chromium/` 下即可被自动发现；
//! - 系统已安装的 Edge / Chrome / Chromium。
//!
//! 维护者注意：校园网未认证时无法联网下载浏览器，所以“拖到现场再下载”的方案不可用。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use chromiumoxide::Page;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::emulation::{
    SetDeviceMetricsOverrideParams, SetLocaleOverrideParams,
};
use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
use futures::StreamExt;
use tokio::task::JoinHandle;

use crate::config::{self, AppConfig, BrowserMode};
use crate::events::EventSink;

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                              AppleWebKit/537.36 (KHTML, like Gecko) \
                              Chrome/124.0.0.0 Safari/537.36";

#[derive(Clone, Debug)]
pub struct ResolvedBrowser {
    pub path: PathBuf,
    pub source: &'static str,
}

impl ResolvedBrowser {
    pub fn description(&self) -> String {
        format!("{}（{}）", self.path.display(), self.source)
    }
}

/// 按配置解析浏览器路径。
pub fn resolve(cfg: &AppConfig) -> Result<ResolvedBrowser, String> {
    match cfg.browser_mode {
        BrowserMode::Auto => bundled_browser()
            .map(|path| ResolvedBrowser {
                path,
                source: "随包 Chromium",
            })
            .or_else(|| {
                system_browser().map(|path| ResolvedBrowser {
                    path,
                    source: "系统浏览器",
                })
            })
            .ok_or_else(|| {
                "未找到可用的 Chromium。请安装 Chrome/Edge，或使用内置 Chromium 的分发包，\
                 也可在设置中指定可执行文件路径。"
                    .to_string()
            }),
        BrowserMode::System => system_browser()
            .map(|path| ResolvedBrowser {
                path,
                source: "系统浏览器",
            })
            .ok_or_else(|| {
                "未找到系统 Chrome/Edge/Chromium。请先安装，或改用“自动/随包”模式。".to_string()
            }),
        BrowserMode::Bundled => bundled_browser()
            .map(|path| ResolvedBrowser {
                path,
                source: "随包 Chromium",
            })
            .ok_or_else(|| {
                "未在程序目录找到随包 Chromium。请使用内置浏览器版本，或改用系统浏览器模式。"
                    .to_string()
            }),
        BrowserMode::Custom => {
            let path = PathBuf::from(cfg.custom_browser_path.trim());
            if path.is_file() && is_executable(&path) {
                Ok(ResolvedBrowser {
                    path,
                    source: "自定义路径",
                })
            } else {
                Err(format!(
                    "自定义浏览器路径无效或不可执行：{}",
                    path.display()
                ))
            }
        }
    }
}

fn bundled_browser() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("CMCC_BUNDLED_CHROMIUM") {
        let path = PathBuf::from(custom);
        if path.is_file() && is_executable(&path) {
            return Some(path);
        }
        if let Some(found) = find_in_dir(&path, 4) {
            return Some(found);
        }
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))?;
    let candidates = [
        exe_dir.join("chromium"),
        exe_dir.join("chrome-headless-shell"),
        exe_dir.join("browser"),
        exe_dir.join("resources").join("chromium"),
    ];
    for dir in candidates {
        if let Some(found) = find_in_dir(&dir, 4) {
            return Some(found);
        }
    }
    None
}

/// 在目录里递归寻找浏览器可执行文件，优先 headless shell。
fn find_in_dir(root: &Path, max_depth: usize) -> Option<PathBuf> {
    if !root.is_dir() {
        if root.is_file() && is_executable(root) && looks_like_browser(root) {
            return Some(root.to_path_buf());
        }
        return None;
    }

    let mut found = Vec::new();
    collect_browsers(root, max_depth, &mut found);
    found.sort_by_key(|path| {
        let name = file_name_lower(path);
        let score = if name.contains("headless") {
            0
        } else if name == "chrome" || name == "chrome.exe" {
            1
        } else if name.starts_with("chromium") {
            2
        } else {
            3
        };
        (score, path.as_os_str().len())
    });
    found.into_iter().next()
}

fn collect_browsers(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_browsers(&path, depth - 1, out);
        } else if file_type.is_file() && looks_like_browser(&path) && is_executable(&path) {
            out.push(path);
        }
    }
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn looks_like_browser(path: &Path) -> bool {
    matches!(
        file_name_lower(path).as_str(),
        "chrome"
            | "chrome.exe"
            | "chromium"
            | "chromium.exe"
            | "msedge"
            | "msedge.exe"
            | "chrome-headless-shell"
            | "chrome-headless-shell.exe"
    )
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn system_browser() -> Option<PathBuf> {
    for var in ["CMCC_CHROME_PATH", "CHROME_PATH"] {
        if let Some(path) = std::env::var_os(var) {
            let path = PathBuf::from(path);
            if path.is_file() && is_executable(&path) {
                return Some(path);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let mut roots = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(var) {
                roots.push(PathBuf::from(root));
            }
        }
        for root in roots {
            for rel in [
                &["Microsoft", "Edge", "Application", "msedge.exe"][..],
                &["Google", "Chrome", "Application", "chrome.exe"][..],
                &["Chromium", "Application", "chrome.exe"][..],
            ] {
                let path = rel.iter().fold(root.clone(), |p, part| p.join(part));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for name in [
            "chromium",
            "chromium-browser",
            "google-chrome",
            "google-chrome-stable",
            "microsoft-edge",
            "microsoft-edge-stable",
        ] {
            if let Some(path) = find_in_path(name) {
                return Some(path);
            }
        }
        for candidate in [
            "/snap/bin/chromium",
            "/usr/lib/chromium/chromium",
            "/opt/google/chrome/chrome",
            "/opt/microsoft/msedge/msedge",
        ] {
            let path = PathBuf::from(candidate);
            if path.is_file() && is_executable(&path) {
                return Some(path);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        for candidate in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ] {
            let path = PathBuf::from(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file() && is_executable(path))
}

/// 一次认证会话持有的浏览器与页面。
pub struct BrowserSession {
    pub browser: Browser,
    pub page: Page,
    handler: JoinHandle<()>,
    profile_dir: Option<PathBuf>,
}

impl BrowserSession {
    /// 正常关闭浏览器并清理临时 profile。
    pub async fn shutdown(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(5), self.browser.close()).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), self.browser.wait()).await;
        self.handler.abort();
        if let Some(dir) = self.profile_dir.take() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

/// 同一进程内记住沙箱不可用，避免每次认证都先失败一次。
static SANDBOX_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

pub async fn launch(resolved: &ResolvedBrowser, sink: &EventSink) -> Result<BrowserSession> {
    let env_override = std::env::var_os("CMCC_NO_SANDBOX").is_some();
    let remembered = SANDBOX_UNAVAILABLE.load(Ordering::Relaxed);
    let force_no_sandbox = env_override || remembered;
    match launch_once(resolved, sink, force_no_sandbox).await {
        Ok(session) => Ok(session),
        Err(err) if !force_no_sandbox && cfg!(target_os = "linux") && is_sandbox_error(&err) => {
            SANDBOX_UNAVAILABLE.store(true, Ordering::Relaxed);
            sink.warn(
                "⚠ Chromium 沙箱不可用（Ubuntu 23.10+ 等系统默认限制 unprivileged user namespaces），\
                 自动以 --no-sandbox 重试；这会降低浏览器的进程隔离性",
            );
            launch_once(resolved, sink, true).await
        }
        Err(err) => Err(err),
    }
}

fn is_sandbox_error(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_ascii_lowercase();
    text.contains("no usable sandbox")
        || text.contains("suid sandbox")
        || text.contains("namespace sandbox")
}

async fn launch_once(
    resolved: &ResolvedBrowser,
    sink: &EventSink,
    no_sandbox: bool,
) -> Result<BrowserSession> {
    sink.info(format!("启动浏览器：{}", resolved.description()));
    let path = &resolved.path;
    let headless_shell = file_name_lower(path).contains("headless");

    let profile_dir = if cfg!(target_os = "windows") || !path.to_string_lossy().contains("/snap/") {
        let dir = config::data_dir().join(format!("chromium-profile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).ok();
        Some(dir)
    } else {
        // snap 版 Chromium 的 confinement 对自定义 profile 目录不友好，交给它自己管理
        None
    };

    // 注意：chromiumoxide 的 `.arg()` 会自动补 `--`，参数不要带前导短横线。
    // `.hide()` 负责加 `--disable-blink-features=AutomationControlled`；
    // 其余 no-first-run / password-store / disable-dev-shm-usage 已在默认参数中。
    let mut builder = BrowserConfig::builder()
        .chrome_executable(path)
        .launch_timeout(Duration::from_secs(30))
        .hide()
        .arg("no-proxy-server")
        .arg("no-default-browser-check");

    builder = if headless_shell {
        // chrome-headless-shell 本身就是无头程序，不能再传 --headless
        builder.with_head()
    } else {
        builder.new_headless_mode()
    };

    if no_sandbox {
        if std::env::var_os("CMCC_NO_SANDBOX").is_some() {
            sink.warn("已按 CMCC_NO_SANDBOX 关闭 Chromium 沙箱");
        }
        builder = builder.no_sandbox();
    }

    if let Some(dir) = &profile_dir {
        builder = builder.user_data_dir(dir);
    }

    let config = builder
        .build()
        .map_err(|err| anyhow::anyhow!("浏览器启动配置无效：{err}"))?;

    let (browser, mut handler) = Browser::launch(config)
        .await
        .context("启动 Chromium 失败")?;
    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let page = browser
        .new_page("about:blank")
        .await
        .context("创建浏览器页面失败")?;

    // 只覆盖 UA，不要启用 chromiumoxide 的 stealth 模式（enable_stealth_mode_with_agent）：
    // 它注入的 window.chrome / navigator.plugins / WebGL 补丁会被 Portal 的瑞数（Riversec）
    // 动态防护识别，挑战脚本执行后返回 400 空页面，导致“找不到认证 Frame”。实网已复现：
    // 默认参数 + stealth → 空页面；去掉 stealth、仅覆盖 UA → 正常进入 Portal。
    // navigator.webdriver 由 `.hide()` 的 --disable-blink-features=AutomationControlled 关闭。
    // 不能把 Linux 浏览器硬伪装成 Windows：UA、navigator.platform、Client Hints 会互相矛盾。
    let native_ua = page.user_agent().await.unwrap_or_default();
    let ua = sanitized_user_agent(&native_ua);

    // chromiumoxide 默认带 --lang=en_US；用 CDP 覆盖 UA、语言与 Accept-Language，
    // 避免 Portal 按语言返回英文结果页导致“登录成功”判断失败。
    let locale = SetLocaleOverrideParams::builder().locale("zh-CN").build();
    if let Err(err) = page.execute(locale).await {
        sink.warn(format!("设置浏览器语言失败（忽略）：{err}"));
    }
    match SetUserAgentOverrideParams::builder()
        .user_agent(&ua)
        .accept_language("zh-CN,zh;q=0.9,en;q=0.8")
        .build()
    {
        Ok(params) => {
            if let Err(err) = page.execute(params).await {
                sink.warn(format!("设置 UA/Accept-Language 失败（忽略）：{err}"));
            }
        }
        Err(err) => sink.warn(format!("构造 UA/Accept-Language 参数失败（忽略）：{err}")),
    }

    // 视口与原版 Playwright 接近，避免 800x600 与指纹中的屏幕尺寸过于异常
    let metrics = SetDeviceMetricsOverrideParams::new(1280, 800, 1.0, false);
    if let Err(err) = page.execute(metrics).await {
        sink.warn(format!("设置视口失败（忽略）：{err}"));
    }

    Ok(BrowserSession {
        browser,
        page,
        handler: handler_task,
        profile_dir,
    })
}

fn sanitized_user_agent(native_ua: &str) -> String {
    if native_ua.is_empty() {
        return USER_AGENT.to_string();
    }
    native_ua
        .replace("HeadlessChrome", "Chrome")
        .replace("HeadlessEdg", "Edg")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_headless_ua() {
        let ua = sanitized_user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) HeadlessChrome/156.0.8078.73 Safari/537.36",
        );
        assert!(ua.contains("Chrome/156.0.8078.73"));
        assert!(!ua.contains("Headless"));
        assert!(ua.contains("X11; Linux x86_64"));
    }

    #[test]
    fn sanitizes_headless_edge_ua() {
        let ua = sanitized_user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) HeadlessEdg/156.0.0.0 Chrome/156.0.0.0 Safari/537.36",
        );
        assert!(ua.contains("Edg/156.0.0.0"));
        assert!(!ua.contains("Headless"));
    }

    #[test]
    fn falls_back_without_native_ua() {
        assert_eq!(sanitized_user_agent(""), USER_AGENT);
    }
}
