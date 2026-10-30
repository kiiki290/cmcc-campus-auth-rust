//! 把后台 `EngineEvent` 批量转发给前端。
//!
//! 使用独立线程消费 `std::sync::mpsc`：空闲时零唤醒，突发日志按 80ms / 200 条合并，
//! 避免每条日志一次 IPC。日志同时写入内存环形缓冲，前端重载后可由 `attach_events` 补齐。

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::events::{EngineEvent, LogLevel, Phase};
use crate::state::{AppState, Snapshot};

const MAX_LOG_LINES: usize = 1500;
const BATCH_WINDOW: Duration = Duration::from_millis(80);
const MAX_BATCH_EVENTS: usize = 200;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    pub id: u64,
    pub time: String,
    pub level: LogLevel,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPayload {
    pub logs: Vec<LogRecord>,
    /// 仅在状态变化时携带，减少 IPC 体积。
    pub snapshot: Option<Snapshot>,
}

pub fn spawn(app: tauri::AppHandle, rx: Receiver<EngineEvent>) {
    let _ = std::thread::Builder::new()
        .name("engine-events".into())
        .spawn(move || run(app, rx));
}

fn run(app: tauri::AppHandle, rx: Receiver<EngineEvent>) {
    loop {
        let Ok(first) = rx.recv() else {
            break;
        };
        let mut events = vec![first];
        let deadline = Instant::now() + BATCH_WINDOW;
        while events.len() < MAX_BATCH_EVENTS {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(event) => events.push(event),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    apply(&app, events);
                    return;
                }
            }
        }
        apply(&app, events);
    }
}

fn apply(app: &tauri::AppHandle, events: Vec<EngineEvent>) {
    use tauri::Manager;

    let state = app.state::<AppState>();
    let mut logs = Vec::new();
    let mut state_dirty = false;
    let mut show_window = false;
    let mut tray_dirty = false;

    {
        let Ok(mut snapshot_guard) = state.snapshot.lock() else {
            return;
        };
        let Ok(mut store) = state.logs.lock() else {
            return;
        };
        let snapshot = &mut *snapshot_guard;

        for event in events {
            match event {
                EngineEvent::Log(line) => {
                    let record = LogRecord {
                        id: store.next_id,
                        time: line.time,
                        level: line.level,
                        text: line.text,
                    };
                    store.next_id += 1;
                    if store.entries.len() >= MAX_LOG_LINES {
                        store.entries.pop_front();
                    }
                    store.entries.push_back(record.clone());
                    logs.push(record);
                }
                EngineEvent::Phase(phase) => {
                    if phase == Phase::Probing || phase == Phase::Idle {
                        // 新一轮探测开始 / 停止监控：清掉上一轮的阶段进度
                        snapshot.stage = None;
                    }
                    snapshot.phase = phase;
                    state_dirty = true;
                }
                EngineEvent::Stage(stage) => {
                    snapshot.stage = Some(stage);
                    state_dirty = true;
                }
                EngineEvent::MonitorActive(active) => {
                    snapshot.monitoring = active;
                    state_dirty = true;
                    tray_dirty = true;
                }
                EngineEvent::Busy(busy) => {
                    snapshot.busy = busy;
                    state_dirty = true;
                    tray_dirty = true;
                }
                EngineEvent::WanIp(ip) => {
                    snapshot.wan_ip = ip;
                    state_dirty = true;
                }
                EngineEvent::Ticked(time) => {
                    snapshot.last_check = Some(time);
                    state_dirty = true;
                }
                EngineEvent::BrowserResolved(info) => {
                    snapshot.browser = Some(info);
                    state_dirty = true;
                }
                EngineEvent::Fatal(message) => {
                    snapshot.fatal = Some(message);
                    state_dirty = true;
                    show_window = true;
                }
            }
        }
    }

    if logs.is_empty() && !state_dirty {
        return;
    }

    let snapshot = state.snapshot();
    let payload = EventPayload {
        logs,
        snapshot: state_dirty.then_some(snapshot),
    };
    if let Ok(channel) = state.channel.lock()
        && let Some(channel) = channel.as_ref()
    {
        let _ = channel.send(payload);
    }

    if tray_dirty {
        crate::tray::update_menu_state(app);
    }
    if show_window {
        crate::window::show_main(app);
    }
}
