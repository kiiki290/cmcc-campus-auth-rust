# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

校园宽带自动认证 GUI：Rust + egui 重写自同目录的 Python CLI 项目（`../cmcc-campus-auth`）。
面向 Windows + Linux；认证核心依赖真实 Chromium 内核（Portal 前的瑞数动态防护拦截纯 HTTP POST）。
全部注释、日志、界面为中文；无 lint 配置；实网已验证单次认证全流程（2026-10-29，见 README）。

## 常用命令

```bash
cargo build                     # 开发构建
cargo run                       # 运行 GUI（Linux 需要 Wayland/X11 + GL）
cargo test                      # 单元测试（23 个，秒级；另有 4 个 ignored）
cargo fmt                       # 格式化
cargo clippy -- -D warnings     # 静态检查

# 需要浏览器的端到端测试（默认 ignore）
CMCC_BUNDLED_CHROMIUM=/path/to/chrome-headless-shell CMCC_ALLOW_LOOPBACK=1 \
    cargo test mock_portal_auth_flow -- --ignored --nocapture
CMCC_TEST_CAPTCHA=/path/to/captcha.png \
    cargo test ocr_smoke -- --ignored --nocapture
cargo test keyring_roundtrip -- --ignored
HOME=$(mktemp -d) CARGO_HOME=/home/zhou/.cargo cargo test autostart_roundtrip -- --ignored

# 打包（Linux 生成 lite/bundled 两个 tar.gz；Windows 用 ps1）
scripts/package.sh
```

首次构建由 `ort` 的 `download-binaries` 下载 onnxruntime 预编译库，需要联网。

## 架构

单 crate 二进制，模块划分见 README「代码结构」。线程模型：

- eframe 主线程：`App::logic` 处理事件/托盘/状态，`App::ui` 只绘图（egui 0.35 的
  `App` trait 是 `logic` + `ui`，不是旧的 `update`）
- 后台 tokio 多线程运行时：认证/监控任务；通过 `std::sync::mpsc` 的
  `EngineEvent` 回传日志与状态；界面每 250ms 主动重绘以消费事件
- 浏览器：`chromiumoxide`（CDP），每次认证启动一个 headless Chromium，结束后关闭

## 认证核心不可随意改动的点

- **必须由浏览器 JS `form.submit()` 提交**：Portal 前的瑞数（Riversec）动态防护会校验；
  纯 HTTP POST 即使 cookie/header/body 完全一致也返回 202。不要为了“轻量化”改成 reqwest/ureq。
- **探测只用黑洞 IP `192.0.2.1`**：未认证时网关 302 到 Portal；已在线时无应答→超时。
  不要把轮询改回 `1.1.1.1`（旧版方案，会给外部服务加压力）。`1.1.1.1` 只用于认证后
  毫秒级放行确认。
- **定位认证 frame**：优先 URL 含 `unionautologin.do`，找不到回落 `frames[1]`；
  Portal 调整 frameset 结构时索引会错位，勿改回硬编码。
- **JSESSIONID**：优先从 form action 的 `;jsessionid=` 提取，其次 cookie，再退回 frame URL。
  提交前若 form action 不含 jsessionid，需要注入。
- **提交后结果**：页面 `title=登录成功/登录失败` 或 `#admin` 文本；致命错误列表见
  `auth.rs::FATAL_PATTERNS`，命中后不重试。
- **放行确认**：成功页 ≠ BRAS 放行，按 0/1/2/3/5/8s 递增用 `1.1.1.1` 确认，
  `None`（探测异常）按本轮失败处理，不要当成“未放行”或成功。
- **失败策略**：单次认证 3 轮失败结束；监控模式 3 轮失败后等待 30 秒继续轮询，
  仅 Fatal 才停止监控。`Fatal` 事件会强制显示主窗口，避免托盘后台无提示。
- **提交目标校验**：默认只允许 `218.200.239.185:8888`，并要求 frame 文档与
  action 同源；回环仅 debug/test 或 `CMCC_ALLOW_LOOPBACK=1`，其他 Portal 用
  `CMCC_PORTAL_HOST=host[:port]` 扩展白名单。校验失败按 Fatal 处理、不重试。
  不要移除该校验，否则伪造 302 可窃取密码。
- **OCR 模型**：`assets/models/common_old.onnx`（约 13MB），与 Python ddddocr 默认模型
  一致；用 `ModelMetadata::from_builtin_old(..., Normalization::ZeroToOne)`。不要换成
  beta/common.onnx 而不改字符集/归一化。
- **Portal 根地址**：从 302 的 outer URL 推导 `/portalserver` 前缀（`portal_base`），
  可用 `CMCC_PORTAL_BASE` 覆盖；原版把验证码 URL 硬编码到同一常量，是双源隐患。

## 浏览器与平台

- 浏览器来源顺序：`CMCC_BUNDLED_CHROMIUM` → 程序目录随包 → 系统 Edge/Chrome/Chromium。
  “自定义路径”是独立模式，不参与自动解析。`chrome-headless-shell` 启动时不能再传 `--headless`（用 `with_head()`）。
- chromiumoxide `.arg()` 会自动补 `--`，参数不要带前导短横线；`.hide()` 负责加
  `--disable-blink-features=AutomationControlled`，另加 `no-proxy-server` /
  `no-default-browser-check`。
- UA 使用浏览器真实 UA 去掉 `HeadlessChrome`/`HeadlessEdg` 标记，不要硬伪装 Windows
  （UA 与 navigator.platform/Client Hints 会矛盾）。chromiumoxide 默认 `--lang=en_US`，
  通过 CDP `Emulation.setLocaleOverride` + `Network.setUserAgentOverride` 设置 `zh-CN`
  和 Accept-Language。
- **不要启用 `enable_stealth_mode_with_agent`**：它注入的 `window.chrome` /
  `navigator.plugins` / WebGL 补丁会被瑞数识别，挑战脚本执行后返回 400 空页面，
  表现为“找不到认证 Frame”（2026-10-29 实网复现）。只覆盖 UA 即可，
  `navigator.webdriver` 由 `.hide()` 的 `--disable-blink-features=AutomationControlled` 关闭。
- 默认参数已含 `--password-store=basic`、`--disable-dev-shm-usage`；`CMCC_NO_SANDBOX=1`
  可强制关闭沙箱。Ubuntu 23.10+ 的 AppArmor userns 限制会让随包 headless shell 报
  “No usable sandbox”，`launch()` 会自动以 `--no-sandbox` 重试并用静态标志记住，
  避免每次认证都失败一次；改这里时注意保留该降级路径。
- 凭据：`keyring`（v1 API，默认 zbus Secret Service / Windows 原生）。配置文件只存学号等，
  密码绝不写盘；测试用独立 service 名，勿覆盖真实条目。
- 托盘：`tray-icon` 的 `ksni` feature（免 GTK/libappindicator 依赖）。Windows 上托盘
  必须在事件循环线程创建；当前在 `App::logic` 首次调用时创建，eframe 正运行在该线程。
  GNOME 需要 AppIndicator/Ubuntu AppIndicators 扩展才能显示。
- 单实例：配置文件目录下 `instance.lock` 文件锁 + `instance.port` 回环端口唤起。
  监听端必须先读掉客户端数据再关闭（否则内核可能回 RST），并设 2 秒读超时，
  防止本地进程连上不发数据卡住监听线程。
- 托盘可用性：ksni 后端在无 StatusNotifierWatcher 时也返回成功，创建前用
  `busctl`/`gdbus` 探测，缺失则禁用关闭到托盘并提示启用 GNOME AppIndicator 扩展。
- 取消：`auth_once` 的 CancelFlag 保存于 Engine，界面提供“取消认证”；长等待
  （浏览器启动/goto/fetch）仍可能延迟数秒，监控停止同理。

## 测试与验证

- 单元测试覆盖 HTTP 头解析、Portal URL 推导、jsessionid 提取、配置 clamp 等。
- `mock_portal_auth_flow`：本地 mock Portal（frameset、表单、验证码、POST 结果）+
  真实 chrome-headless-shell，验证 探测→OCR→提交→结果→放行 全链路。运行前需设置
  `CMCC_BUNDLED_CHROMIUM`；测试内部设置 `CMCC_PROBE_TARGET` / `CMCC_RELEASE_TARGET` /
  `CMCC_NO_SANDBOX`，必须单独运行（环境变量是进程级的）。
- UI 截图调试：`cargo run -- --screenshot /tmp/app.png`（渲染数帧后保存并退出），
  已用于验证中文字体/布局。开发时用 `XDG_CONFIG_HOME`/`XDG_DATA_HOME` 指向临时目录，
  避免污染真实配置。
- **实网验证状态**：2026-10-29 已在真实校园网用系统 Edge 完成单次认证全流程
  （探测→frame→OCR→JS 提交→结果→BRAS 放行）；瑞数挑战失败问题已复现并修复。
  断线自动重连的长时间稳定性仍建议观察；Portal 改版、放行细节仍只能在实网确认。
  改动认证相关代码后，至少跑 mock 测试，并在 README「已知局限」中如实记录。

## 打包

- `scripts/package.sh [all|lite|bundled]`：Linux release + 两个 tar.gz；
  `scripts/package.ps1`：Windows 对应版本。
- lite 不携带浏览器；bundled 通过 `fetch_chromium.sh|.ps1` 从 Chrome for Testing 下载
  `chrome-headless-shell` 放进 `chromium/`，程序递归查找。
- 模型已 `include_bytes!` 进二进制，无需单独分发 `assets/`。
- 发布产物不要包含 `~/.config`、浏览器 profile、日志。

## 维护约定

- 保持最低 Rust 1.92（eframe 0.35 MSRV 1.92；本机 apt rustc 1.93）。
- 改动认证逻辑时同步检查：重试次数/间隔、致命错误列表、日志文案、README/CLAUDE.md。
- 完成 TODO 项后移入 docs/DONE.md；docs/TODO.md 与 docs/DONE.md 必须与代码实际状态一致。
- 提交前按全局规范做 diff 审查；提交信息用中文 conventional commit。
