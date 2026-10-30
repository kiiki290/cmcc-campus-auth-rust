//! Tauri 命令、托盘与事件桥共享的应用状态。

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::ipc::Channel;

use crate::auth::Credentials;
use crate::bridge::{EventPayload, LogRecord};
use crate::config::AppConfig;
use crate::engine::Engine;
use crate::events::{AuthStage, EventSink, Phase};
use crate::tray::TrayHandles;

/// 前端渲染所需的全部运行状态。字段变化通过 `bridge` 批量推送。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub phase: Phase,
    pub stage: Option<AuthStage>,
    pub busy: bool,
    pub monitoring: bool,
    pub wan_ip: Option<String>,
    pub last_check: Option<String>,
    pub browser: Option<String>,
    pub fatal: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            stage: None,
            busy: false,
            monitoring: false,
            wan_ip: None,
            last_check: None,
            browser: None,
            fatal: None,
        }
    }
}

/// 内存中的日志环形缓冲。即使前端尚未连接，也能在 `attach_events` 时补齐启动日志。
pub struct LogStore {
    pub entries: VecDeque<LogRecord>,
    pub next_id: u64,
}

impl Default for LogStore {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            next_id: 1,
        }
    }
}

pub struct AppState {
    /// `Engine`/`EventSink` 内含 `std::sync::mpsc::Sender`（!Sync），用 `Mutex` 包裹才能放进 Tauri State。
    pub engine: Mutex<Engine>,
    pub sink: Mutex<EventSink>,
    pub config: Mutex<AppConfig>,
    /// 系统密钥环中的密码，仅驻留内存，绝不返回前端。
    pub password: Mutex<Option<String>>,
    pub snapshot: Mutex<Snapshot>,
    pub logs: Mutex<LogStore>,
    pub channel: Mutex<Option<Channel<EventPayload>>>,
    pub tray: Mutex<Option<TrayHandles>>,
    /// true 表示“关闭到托盘”可用（托盘创建成功且宿主可用）。
    pub tray_ok: AtomicBool,
    pub allow_exit: AtomicBool,
    pub ui_ready: AtomicBool,
    pub minimized_start: bool,
    /// 事件转发线程需要，只在启动时取出一次。
    pub rx: Mutex<Option<Receiver<crate::events::EngineEvent>>>,
    /// 保持 tokio 运行时存活；Engine 只持有 Handle。
    pub _runtime: tokio::runtime::Runtime,
}

impl AppState {
    pub fn creds_ready(&self) -> bool {
        let student_id = self
            .config
            .lock()
            .map(|cfg| !cfg.student_id.trim().is_empty())
            .unwrap_or(false);
        let password = self.password.lock().is_ok_and(|pwd| pwd.is_some());
        student_id && password
    }

    /// 供 Engine 使用的凭据快照（密码只在此处离开密钥环缓存）。
    pub fn credentials(&self) -> Option<Arc<Credentials>> {
        let student_id = self.config.lock().ok()?.student_id.clone();
        let password = self.password.lock().ok()?.clone()?;
        if student_id.trim().is_empty() || password.is_empty() {
            return None;
        }
        Some(Arc::new(Credentials {
            student_id,
            password,
        }))
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }

    /// 取最近 `limit` 条日志（按时间顺序）。
    pub fn recent_logs(&self, limit: usize) -> Vec<LogRecord> {
        let Ok(store) = self.logs.lock() else {
            return Vec::new();
        };
        let skip = store.entries.len().saturating_sub(limit);
        store.entries.iter().skip(skip).cloned().collect()
    }

    pub fn log_info(&self, text: impl Into<String>) {
        if let Ok(sink) = self.sink.lock() {
            sink.info(text);
        }
    }

    pub fn log_warn(&self, text: impl Into<String>) {
        if let Ok(sink) = self.sink.lock() {
            sink.warn(text);
        }
    }

    pub fn log_error(&self, text: impl Into<String>) {
        if let Ok(sink) = self.sink.lock() {
            sink.error(text);
        }
    }

    pub fn log_success(&self, text: impl Into<String>) {
        if let Ok(sink) = self.sink.lock() {
            sink.success(text);
        }
    }
}

/// 供前端 `attach_events` 返回的完整初始数据。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub version: String,
    pub config: AppConfig,
    pub has_password: bool,
    pub masked_student_id: String,
    pub snapshot: Snapshot,
    pub logs: Vec<LogRecord>,
    pub config_path: String,
    /// 关闭到托盘是否可用（托盘创建成功且系统宿主可用）。
    pub tray_ok: bool,
}

impl Bootstrap {
    pub fn collect(state: &AppState) -> Self {
        let config = state
            .config
            .lock()
            .map(|cfg| cfg.clone())
            .unwrap_or_default();
        let has_password = state.password.lock().is_ok_and(|pwd| pwd.is_some());
        let masked_student_id = Credentials {
            student_id: config.student_id.clone(),
            password: String::new(),
        }
        .masked_student_id();
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            config,
            has_password,
            masked_student_id,
            snapshot: state.snapshot(),
            logs: state.recent_logs(300),
            config_path: crate::config::config_path().display().to_string(),
            tray_ok: state.tray_ok.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_serializes_for_frontend() {
        let snapshot = Snapshot {
            phase: Phase::Authenticating,
            stage: Some(AuthStage::CaptchaReady),
            busy: true,
            wan_ip: Some("10.0.0.1".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["phase"], "authenticating");
        assert_eq!(json["stage"], "captcha_ready");
        assert_eq!(json["wanIp"], "10.0.0.1");
        assert_eq!(json["busy"], true);
        assert_eq!(json["monitoring"], false);
        assert!(json.get("fatal").is_some());
    }
}
