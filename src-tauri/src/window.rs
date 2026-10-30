//! 主窗口显隐辅助。

use tauri::{AppHandle, Manager};

/// 显示并聚焦主窗口；窗口被托盘隐藏或被最小化时都能恢复。
pub fn show_main(app: &AppHandle) {
    // 单实例回调可能在 setup 完成前触发，此时 AppState 尚未注册，不能用 state()。
    let state = app.try_state::<crate::state::AppState>();
    let Some(window) = app.get_webview_window("main") else {
        if let Some(state) = state.as_ref() {
            state.log_warn("show_main：找不到主窗口");
        }
        return;
    };
    if let Err(err) = window.show()
        && let Some(state) = state.as_ref()
    {
        state.log_warn(format!("show 失败：{err}"));
    }
    if let Err(err) = window.unminimize()
        && let Some(state) = state.as_ref()
    {
        state.log_warn(format!("unminimize 失败：{err}"));
    }
    if let Err(err) = window.set_focus()
        && let Some(state) = state.as_ref()
    {
        state.log_warn(format!("set_focus 失败：{err}"));
    }
}

/// 启动阶段决定是否显示窗口：`--minimized` 且托盘可用时保持隐藏。
pub fn show_on_startup(app: &AppHandle) {
    let state = app.state::<crate::state::AppState>();
    let minimized = state.minimized_start;
    let tray_ok = state.tray_ok.load(std::sync::atomic::Ordering::Relaxed);
    if minimized && tray_ok {
        return;
    }
    if minimized && !tray_ok {
        state.log_warn("启动时要求最小化，但系统托盘不可用，已直接显示窗口");
    }
    state.log_info("启动阶段显示主窗口");
    show_main(app);
}
