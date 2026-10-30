//! 前端 IPC 命令层。
//!
//! 业务规则全部在 Rust 侧：前端只能提交设置草稿、触发动作、请求快照。
//! 密码只以 `Option<String>` 草稿形式进入，绝不返回前端。

use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::autostart;
use crate::bridge::EventPayload;
use crate::browser;
use crate::config::{AppConfig, BrowserMode, MAX_POLL_INTERVAL, MIN_POLL_INTERVAL};
use crate::credentials;
use crate::state::{AppState, Bootstrap};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsInput {
    pub student_id: String,
    /// `None` 或空字符串表示“保持已保存的密码不变”。
    pub password: Option<String>,
    pub poll_interval: u64,
    pub browser_mode: BrowserMode,
    pub custom_browser_path: String,
    pub close_to_tray: bool,
    pub auto_start_monitor: bool,
    pub autostart: bool,
}

impl SettingsInput {
    fn apply_to(&self, base: &AppConfig) -> AppConfig {
        let mut cfg = base.clone();
        cfg.student_id = self.student_id.trim().to_string();
        cfg.poll_interval = self
            .poll_interval
            .clamp(MIN_POLL_INTERVAL, MAX_POLL_INTERVAL);
        cfg.browser_mode = self.browser_mode;
        cfg.custom_browser_path = self.custom_browser_path.trim().to_string();
        cfg.close_to_tray = self.close_to_tray;
        cfg.auto_start_monitor = self.auto_start_monitor;
        cfg.autostart = self.autostart;
        cfg
    }

    fn browser_config(&self, base: &AppConfig) -> AppConfig {
        let mut cfg = base.clone();
        cfg.browser_mode = self.browser_mode;
        cfg.custom_browser_path = self.custom_browser_path.trim().to_string();
        cfg
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupHint {
    /// 启动时自动监控但没有可用凭据，前端应跳到设置页并提示。
    pub needs_setup: bool,
}

// ── 动作实现（命令与托盘共用） ───────────────────────────────────────────────

pub(crate) fn do_auth_now(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let Some(creds) = state.credentials() else {
        return Err("请先填写并保存学号与密码".to_string());
    };
    let cfg = state
        .config
        .lock()
        .map(|cfg| cfg.clone())
        .unwrap_or_default();
    let result = state
        .engine
        .lock()
        .map_err(|_| "内部状态异常".to_string())?
        .auth_once(creds, cfg);
    if let Err(err) = &result {
        state.log_warn(err.clone());
    }
    result
}

/// 供托盘、命令和启动兜底共用。
pub fn do_start_monitor(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let Some(creds) = state.credentials() else {
        return Err("请先填写并保存学号与密码".to_string());
    };
    let cfg = state
        .config
        .lock()
        .map(|cfg| cfg.clone())
        .unwrap_or_default();
    let result = state
        .engine
        .lock()
        .map_err(|_| "内部状态异常".to_string())?
        .start_monitor(creds, cfg);
    if let Err(err) = &result {
        state.log_warn(err.clone());
    }
    result
}

pub(crate) fn do_stop_monitor(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(engine) = state.engine.lock() {
        engine.stop_monitor();
    }
}

pub(crate) fn do_cancel_auth(app: &AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(engine) = state.engine.lock() {
        engine.cancel_auth();
    }
}

pub(crate) fn do_quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.allow_exit.store(true, Ordering::SeqCst);
    if let Ok(engine) = state.engine.lock() {
        engine.stop_monitor();
    }
    app.exit(0);
}

pub(crate) fn report_action(app: &AppHandle, result: Result<(), String>) {
    if let Err(err) = result {
        app.state::<AppState>().log_warn(err);
    }
}

// ── 命令 ────────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn attach_events(state: State<'_, AppState>, channel: Channel<EventPayload>) -> Bootstrap {
    if let Ok(mut slot) = state.channel.lock() {
        *slot = Some(channel);
    }
    Bootstrap::collect(&state)
}

#[tauri::command]
pub fn ui_ready(app: AppHandle, state: State<'_, AppState>) -> StartupHint {
    state.ui_ready.store(true, Ordering::SeqCst);
    state.log_info("前端界面已就绪");
    crate::window::show_on_startup(&app);

    let auto_start = state
        .config
        .lock()
        .map(|cfg| cfg.auto_start_monitor)
        .unwrap_or(false);
    let mut needs_setup = false;
    if auto_start {
        if state.creds_ready() {
            if !state.snapshot().monitoring {
                let _ = do_start_monitor(&app);
            }
        } else {
            needs_setup = true;
            // 开机自启 + 无凭据：必须显示窗口，否则用户看不到配置提示
            if state.minimized_start {
                crate::window::show_main(&app);
            }
        }
    }
    StartupHint { needs_setup }
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, input: SettingsInput) -> Result<Bootstrap, String> {
    tauri::async_runtime::spawn_blocking(move || save_settings_blocking(&app, input))
        .await
        .map_err(|err| format!("保存设置失败：{err}"))?
}

fn save_settings_blocking(app: &AppHandle, input: SettingsInput) -> Result<Bootstrap, String> {
    let state = app.state::<AppState>();

    let student_id = input.student_id.trim().to_string();
    if student_id.is_empty() {
        return Err("学号不能为空".to_string());
    }

    let current_password = state.password.lock().ok().and_then(|pwd| pwd.clone());
    let provided = input.password.clone().unwrap_or_default();
    let password = if provided.is_empty() {
        current_password.clone()
    } else {
        Some(provided.clone())
    };
    let Some(password) = password else {
        return Err("密码不能为空".to_string());
    };

    // 仅在提供新密码且与已保存的不同时写密钥环
    if !provided.is_empty()
        && current_password.as_deref() != Some(provided.as_str())
        && let Err(err) = credentials::save_password(&provided)
    {
        state.log_error(format!("保存密码失败：{err:#}"));
        return Err(format!("保存密码到系统密钥环失败：{err:#}"));
    }

    let base = state
        .config
        .lock()
        .map(|cfg| cfg.clone())
        .unwrap_or_default();
    let cfg = input.apply_to(&base);

    if cfg.autostart != autostart::is_enabled() {
        match autostart::set_enabled(cfg.autostart) {
            Ok(()) => {
                if cfg.autostart {
                    state.log_success("已启用开机自启（启动时最小化到托盘）");
                } else {
                    state.log_info("已关闭开机自启");
                }
            }
            Err(err) => state.log_warn(err),
        }
    }
    if let Err(err) = cfg.save() {
        state.log_error(format!("保存配置失败：{err:#}"));
        return Err(format!("保存配置失败：{err:#}"));
    }

    if let Ok(mut slot) = state.config.lock() {
        *slot = cfg.clone();
    }
    if let Ok(mut slot) = state.password.lock() {
        *slot = Some(password);
    }
    state.log_success("设置已保存");
    match browser::resolve(&cfg) {
        Ok(resolved) => {
            if let Ok(mut snapshot) = state.snapshot.lock() {
                snapshot.browser = Some(resolved.description());
            }
            state.log_info(format!("浏览器：{}", resolved.description()));
        }
        Err(err) => {
            if let Ok(mut snapshot) = state.snapshot.lock() {
                snapshot.browser = None;
            }
            state.log_warn(format!("浏览器检测：{err}"));
        }
    }

    crate::tray::update_menu_state(app);
    Ok(Bootstrap::collect(&state))
}

#[tauri::command]
pub fn auth_now(app: AppHandle) -> Result<(), String> {
    do_auth_now(&app)
}

#[tauri::command]
pub fn start_monitor(app: AppHandle) -> Result<(), String> {
    do_start_monitor(&app)
}

#[tauri::command]
pub fn stop_monitor(app: AppHandle) {
    do_stop_monitor(&app);
}

#[tauri::command]
pub fn cancel_auth(app: AppHandle) {
    do_cancel_auth(&app);
}

#[tauri::command]
pub async fn test_browser(app: AppHandle, input: SettingsInput) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let base = state
            .config
            .lock()
            .map(|cfg| cfg.clone())
            .unwrap_or_default();
        let cfg = input.browser_config(&base);
        match browser::resolve(&cfg) {
            Ok(resolved) => {
                if let Ok(mut snapshot) = state.snapshot.lock() {
                    snapshot.browser = Some(resolved.description());
                }
                state.log_success(format!("浏览器可用：{}", resolved.description()));
                Ok(resolved.description())
            }
            Err(err) => {
                if let Ok(mut snapshot) = state.snapshot.lock() {
                    snapshot.browser = None;
                }
                state.log_error(format!("浏览器不可用：{err}"));
                Err(err)
            }
        }
    })
    .await
    .map_err(|err| format!("浏览器检测失败：{err}"))?
}

#[tauri::command]
pub async fn clear_credentials(app: AppHandle) -> Result<Bootstrap, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        do_stop_monitor(&app);
        do_cancel_auth(&app);

        if let Err(err) = credentials::delete_password() {
            state.log_warn(format!("删除系统密钥环中的密码失败：{err:#}"));
        }
        if let Ok(mut slot) = state.password.lock() {
            *slot = None;
        }

        let mut cfg = state
            .config
            .lock()
            .map(|cfg| cfg.clone())
            .unwrap_or_default();
        cfg.student_id.clear();
        if let Err(err) = cfg.save() {
            state.log_error(format!("保存配置失败：{err:#}"));
        }
        if let Ok(mut slot) = state.config.lock() {
            *slot = cfg;
        }

        state.log_success("已清除已保存的账号密码");
        crate::tray::update_menu_state(&app);
        Ok(Bootstrap::collect(&state))
    })
    .await
    .map_err(|err| format!("清除账号密码失败：{err}"))?
}

#[tauri::command]
pub fn dismiss_fatal(state: State<'_, AppState>) {
    if let Ok(mut snapshot) = state.snapshot.lock() {
        snapshot.fatal = None;
    }
}

/// 同步命令会跑在主线程，而 `blocking_pick_file` 会阻塞等待主线程弹窗，二者叠加会死锁；
/// 放到阻塞线程里等待即可。
#[tauri::command]
pub async fn pick_browser_path(app: AppHandle) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_file()
            .and_then(|file| file.into_path().ok())
            .map(|path| path.display().to_string())
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> AppConfig {
        AppConfig {
            poll_interval: 10,
            ..Default::default()
        }
    }

    #[test]
    fn settings_input_matches_frontend_camel_case() {
        let json = r#"{
            "studentId": "  20231234  ",
            "password": null,
            "pollInterval": 1,
            "browserMode": "system",
            "customBrowserPath": "  /usr/bin/chromium  ",
            "closeToTray": false,
            "autoStartMonitor": false,
            "autostart": true
        }"#;
        let input: SettingsInput = serde_json::from_str(json).unwrap();
        let cfg = input.apply_to(&base());
        assert_eq!(cfg.student_id, "20231234");
        assert_eq!(cfg.poll_interval, MIN_POLL_INTERVAL);
        assert_eq!(cfg.browser_mode, BrowserMode::System);
        assert_eq!(cfg.custom_browser_path, "/usr/bin/chromium");
        assert!(!cfg.close_to_tray);
        assert!(!cfg.auto_start_monitor);
        assert!(cfg.autostart);
    }

    #[test]
    fn settings_input_clamps_upper_bound() {
        let input: SettingsInput = serde_json::from_str(
            r#"{
                "studentId": "1",
                "password": null,
                "pollInterval": 999999,
                "browserMode": "auto",
                "customBrowserPath": "",
                "closeToTray": true,
                "autoStartMonitor": true,
                "autostart": false
            }"#,
        )
        .unwrap();
        assert_eq!(input.apply_to(&base()).poll_interval, MAX_POLL_INTERVAL);
    }
}
