//! 系统托盘：常驻图标与快捷菜单。

use std::sync::mpsc::{Receiver, channel};

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::icon;

#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    Show,
    AuthNow,
    StartMonitor,
    StopMonitor,
    Quit,
}

pub struct TrayState {
    tray: TrayIcon,
    pub rx: Receiver<TrayCommand>,
    show_item: MenuItem,
    auth_item: MenuItem,
    start_item: MenuItem,
    stop_item: MenuItem,
    quit_item: MenuItem,
}

impl TrayState {
    pub fn update_state(&self, monitoring: bool, busy: bool, creds_ready: bool) {
        self.show_item.set_enabled(true);
        self.auth_item
            .set_enabled(creds_ready && !busy && !monitoring);
        self.start_item
            .set_enabled(creds_ready && !busy && !monitoring);
        self.stop_item.set_enabled(monitoring);
        self.quit_item.set_enabled(true);

        let status = if monitoring { "监控中" } else { "待机" };
        let _ = self
            .tray
            .set_tooltip(Some(format!("校园宽带自动认证 — {status}")));
    }
}

/// 检查系统会话总线是否存在 StatusNotifierWatcher。
///
/// tray-icon 的 ksni 后端会假定宿主存在，GNOME 未启用 AppIndicator 扩展时
/// `build()` 仍返回 Ok，但图标不可见。这里先做 D-Bus 探测，避免关闭窗口后
/// 用户没有托盘入口。
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

pub fn create(ctx: egui::Context) -> Result<TrayState, String> {
    let show_item = MenuItem::new("显示主窗口", true, None);
    let auth_item = MenuItem::new("立即认证", true, None);
    let start_item = MenuItem::new("开始监控", true, None);
    let stop_item = MenuItem::new("停止监控", false, None);
    let quit_item = MenuItem::new("退出", true, None);

    let menu = Menu::new();
    let sep1 = PredefinedMenuItem::separator();
    let sep2 = PredefinedMenuItem::separator();
    let items: [&dyn tray_icon::menu::IsMenuItem; 7] = [
        &show_item,
        &auth_item,
        &sep1,
        &start_item,
        &stop_item,
        &sep2,
        &quit_item,
    ];
    for item in items {
        menu.append(item)
            .map_err(|err| format!("创建托盘菜单失败：{err}"))?;
    }

    let (tx, rx) = channel::<TrayCommand>();
    let show_id = show_item.id().clone();
    let auth_id = auth_item.id().clone();
    let start_id = start_item.id().clone();
    let stop_id = stop_item.id().clone();
    let quit_id = quit_item.id().clone();
    let menu_tx = tx.clone();
    let menu_ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let command = if event.id == show_id {
            Some(TrayCommand::Show)
        } else if event.id == auth_id {
            Some(TrayCommand::AuthNow)
        } else if event.id == start_id {
            Some(TrayCommand::StartMonitor)
        } else if event.id == stop_id {
            Some(TrayCommand::StopMonitor)
        } else if event.id == quit_id {
            Some(TrayCommand::Quit)
        } else {
            None
        };
        if let Some(command) = command {
            let _ = menu_tx.send(command);
            menu_ctx.request_repaint();
        }
    }));

    let click_tx = tx;
    let click_ctx = ctx;
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = click_tx.send(TrayCommand::Show);
            click_ctx.request_repaint();
        }
    }));

    let rgba = icon::icon_rgba(32);
    let icon = Icon::from_rgba(rgba, 32, 32).map_err(|err| format!("创建托盘图标失败：{err}"))?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("校园宽带自动认证")
        .with_icon(icon)
        .build()
        .map_err(|err| format!("创建系统托盘失败：{err}"))?;

    Ok(TrayState {
        tray,
        rx,
        show_item,
        auth_item,
        start_item,
        stop_item,
        quit_item,
    })
}
