//! 系统托盘：常驻图标与快捷菜单（Tauri 2 实现）。

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::icon;
use crate::state::AppState;

pub struct TrayHandles {
    pub tray: TrayIcon<Wry>,
    pub auth: MenuItem<Wry>,
    pub start: MenuItem<Wry>,
    pub stop: MenuItem<Wry>,
}

/// 检查系统会话总线是否存在 StatusNotifierWatcher。
///
/// Tauri 的 Linux 托盘依赖 libayatana-appindicator；GNOME 未启用 AppIndicator
/// 扩展时图标不可见。启动前先做 D-Bus 探测，避免关闭窗口后用户没有托盘入口。
#[cfg(target_os = "linux")]
pub fn host_available() -> bool {
    use std::process::Command;

    let mut probed = false;

    if let Ok(output) = Command::new("busctl")
        .args(["--user", "list", "--no-pager"])
        .output()
        && output.status.success()
    {
        probed = true;
        let text = String::from_utf8_lossy(&output.stdout);
        if text.contains("org.kde.StatusNotifierWatcher")
            || text.contains("org.x.StatusNotifierWatcher")
        {
            return true;
        }
    }

    if let Ok(output) = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.NameHasOwner",
            "org.kde.StatusNotifierWatcher",
        ])
        .output()
        && output.status.success()
    {
        probed = true;
        if String::from_utf8_lossy(&output.stdout).contains("true") {
            return true;
        }
    }

    // 无法探测时不阻止创建；能探测但未找到则返回 false
    !probed
}

#[cfg(not(target_os = "linux"))]
pub fn host_available() -> bool {
    true
}

pub fn create(app: &AppHandle) -> Result<TrayHandles, String> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)
        .map_err(|err| format!("创建托盘菜单失败：{err}"))?;
    let auth = MenuItem::with_id(app, "auth", "立即认证", true, None::<&str>)
        .map_err(|err| format!("创建托盘菜单失败：{err}"))?;
    let start = MenuItem::with_id(app, "start", "开始监控", true, None::<&str>)
        .map_err(|err| format!("创建托盘菜单失败：{err}"))?;
    let stop = MenuItem::with_id(app, "stop", "停止监控", false, None::<&str>)
        .map_err(|err| format!("创建托盘菜单失败：{err}"))?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .map_err(|err| format!("创建托盘菜单失败：{err}"))?;
    let sep1 =
        PredefinedMenuItem::separator(app).map_err(|err| format!("创建托盘菜单失败：{err}"))?;
    let sep2 =
        PredefinedMenuItem::separator(app).map_err(|err| format!("创建托盘菜单失败：{err}"))?;

    let menu = Menu::with_items(app, &[&show, &auth, &sep1, &start, &stop, &sep2, &quit])
        .map_err(|err| format!("创建托盘菜单失败：{err}"))?;

    let icon = Image::new_owned(icon::icon_rgba(32), 32, 32);
    let tray = TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("校园宽带自动认证")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => crate::window::show_main(app),
            "auth" => crate::commands::report_action(app, crate::commands::do_auth_now(app)),
            "start" => crate::commands::report_action(app, crate::commands::do_start_monitor(app)),
            "stop" => crate::commands::do_stop_monitor(app),
            "quit" => crate::commands::do_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::window::show_main(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|err| format!("创建系统托盘失败：{err}"))?;

    Ok(TrayHandles {
        tray,
        auth,
        start,
        stop,
    })
}

/// 按当前状态刷新菜单项可用性（与界面按钮规则一致）。
pub fn update_menu_state(app: &AppHandle) {
    let state = app.state::<AppState>();
    let snapshot = state.snapshot();
    let ready = state.creds_ready();
    let idle = ready && !snapshot.busy && !snapshot.monitoring;
    if let Ok(guard) = state.tray.lock()
        && let Some(tray) = guard.as_ref()
    {
        let _ = tray.auth.set_enabled(idle);
        let _ = tray.start.set_enabled(idle);
        let _ = tray.stop.set_enabled(snapshot.monitoring);
        #[cfg(not(target_os = "linux"))]
        {
            let tooltip = if snapshot.monitoring {
                "校园宽带自动认证 — 监控中"
            } else {
                "校园宽带自动认证 — 待机"
            };
            let _ = tray.tray.set_tooltip(Some(tooltip));
        }
    }
}
