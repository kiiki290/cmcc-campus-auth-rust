//! egui 主界面。

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText, ViewportCommand};
use egui::{Align, Layout};

use crate::auth::Credentials;
use crate::autostart;
use crate::browser;
use crate::config::{self, AppConfig, BrowserMode, MAX_POLL_INTERVAL, MIN_POLL_INTERVAL};
use crate::credentials;
use crate::engine::Engine;
use crate::events::{EngineEvent, LogLevel, LogLine, Phase};
use crate::single_instance::InstanceGuard;
use crate::tray::{self, TrayCommand, TrayState};

const MAX_LOG_LINES: usize = 3000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Control,
    Settings,
}

struct SettingsDraft {
    student_id: String,
    password: String,
    poll_interval: u64,
    browser_mode: BrowserMode,
    custom_browser_path: String,
    close_to_tray: bool,
    auto_start_monitor: bool,
    autostart: bool,
}

impl SettingsDraft {
    fn from_config(cfg: &AppConfig, password: &str) -> Self {
        Self {
            student_id: cfg.student_id.clone(),
            password: password.to_string(),
            poll_interval: cfg.poll_interval,
            browser_mode: cfg.browser_mode,
            custom_browser_path: cfg.custom_browser_path.clone(),
            close_to_tray: cfg.close_to_tray,
            auto_start_monitor: cfg.auto_start_monitor,
            autostart: cfg.autostart,
        }
    }
}

pub struct App {
    engine: Engine,
    rx: Receiver<EngineEvent>,
    _instance: Option<InstanceGuard>,
    logs: Vec<LogLine>,
    phase: Phase,
    monitor_on: bool,
    busy: bool,
    wan_ip: Option<String>,
    last_check: Option<String>,
    browser_info: Option<String>,
    fatal: Option<String>,
    notice: Option<String>,
    config: AppConfig,
    password: String,
    settings: SettingsDraft,
    tab: Tab,
    auto_scroll: bool,
    allow_exit: bool,
    tray: Option<TrayState>,
    tray_failed: bool,
    tray_host_available: bool,
    minimized_start: bool,
    started: bool,
    confirm_clear: bool,
    pending_show_window: bool,
    screenshot_path: Option<String>,
    screenshot_requested: bool,
    screenshot_frames: u32,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: AppConfig,
        password: String,
        startup_warnings: Vec<String>,
        engine: Engine,
        rx: Receiver<EngineEvent>,
        instance: Option<InstanceGuard>,
        minimized: bool,
        screenshot_path: Option<String>,
    ) -> Self {
        let cjk_ok = setup_fonts(&cc.egui_ctx);
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
        });

        let actual_autostart = autostart::is_enabled();
        let mut config = config;
        config.autostart = actual_autostart;

        let mut app = Self {
            engine,
            rx,
            _instance: instance,
            logs: Vec::new(),
            phase: Phase::Idle,
            monitor_on: false,
            busy: false,
            wan_ip: None,
            last_check: None,
            browser_info: None,
            fatal: None,
            notice: None,
            settings: SettingsDraft::from_config(&config, &password),
            config,
            password,
            tab: Tab::Control,
            auto_scroll: true,
            allow_exit: false,
            tray: None,
            tray_failed: false,
            tray_host_available: false,
            minimized_start: minimized,
            started: false,
            confirm_clear: false,
            pending_show_window: false,
            screenshot_path,
            screenshot_requested: false,
            screenshot_frames: 0,
        };

        app.log_info(format!(
            "校园宽带自动认证 v{} 启动",
            env!("CARGO_PKG_VERSION")
        ));
        app.log_info(format!("配置目录：{}", config::config_dir().display()));
        if !cjk_ok {
            app.log_warn("未找到系统中文字体，界面可能出现方块字");
        }
        for warning in startup_warnings {
            app.log_warn(warning);
        }
        if !app.config.student_id.is_empty() {
            app.log_info(format!("已加载账号：{}", app.config.student_id));
        }
        if !app.password.is_empty() {
            app.log_info("已从系统密钥环加载密码");
        } else {
            app.log_info("尚未保存密码，请在“设置”中填写账号密码");
        }
        app
    }

    fn log_line(&mut self, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        crate::logging::write_line(level.label(), &text);
        self.logs.push(LogLine {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            level,
            text,
        });
        if self.logs.len() > MAX_LOG_LINES {
            let excess = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(..excess);
        }
    }

    fn log_info(&mut self, text: impl Into<String>) {
        self.log_line(LogLevel::Info, text);
    }

    fn log_warn(&mut self, text: impl Into<String>) {
        self.log_line(LogLevel::Warn, text);
    }

    fn log_error(&mut self, text: impl Into<String>) {
        self.log_line(LogLevel::Error, text);
    }

    fn log_success(&mut self, text: impl Into<String>) {
        self.log_line(LogLevel::Success, text);
    }

    fn creds_ready(&self) -> bool {
        !self.config.student_id.trim().is_empty() && !self.password.is_empty()
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                EngineEvent::Log(line) => {
                    self.logs.push(line);
                    if self.logs.len() > MAX_LOG_LINES {
                        let excess = self.logs.len() - MAX_LOG_LINES;
                        self.logs.drain(..excess);
                    }
                }
                EngineEvent::Phase(phase) => self.phase = phase,
                EngineEvent::MonitorActive(active) => self.monitor_on = active,
                EngineEvent::Busy(busy) => self.busy = busy,
                EngineEvent::WanIp(ip) => self.wan_ip = ip,
                EngineEvent::Ticked(time) => self.last_check = Some(time),
                EngineEvent::BrowserResolved(info) => self.browser_info = Some(info),
                EngineEvent::Fatal(message) => {
                    self.fatal = Some(message);
                    self.pending_show_window = true;
                }
            }
        }
    }

    fn ensure_tray(&mut self, ctx: &egui::Context) {
        if self.tray.is_some() || self.tray_failed {
            return;
        }
        let host_available = tray::host_available();
        self.tray_host_available = host_available;
        if !host_available {
            self.log_warn(
                "未检测到系统托盘宿主（GNOME 请启用 AppIndicator/Ubuntu AppIndicators 扩展）；\
                 将禁用“关闭到托盘”，关闭窗口会直接退出",
            );
        }
        match tray::create(ctx.clone()) {
            Ok(state) => {
                self.tray = Some(state);
                if host_available {
                    self.log_success("系统托盘已启用");
                } else {
                    self.log_warn("托盘服务已创建，但图标可能不可见");
                }
            }
            Err(err) => {
                self.tray_failed = true;
                self.log_warn(format!("{err}；关闭窗口将直接退出"));
            }
        }
    }

    fn drain_tray(&mut self, ctx: &egui::Context) {
        let mut commands = Vec::new();
        if let Some(tray) = &self.tray {
            while let Ok(command) = tray.rx.try_recv() {
                commands.push(command);
            }
        }
        for command in commands {
            match command {
                TrayCommand::Show => self.show_window(ctx),
                TrayCommand::AuthNow => {
                    self.start_auth_once();
                }
                TrayCommand::StartMonitor => {
                    self.start_monitor();
                }
                TrayCommand::StopMonitor => self.stop_monitor(),
                TrayCommand::Quit => self.quit(ctx),
            }
        }

        let mut activations = 0;
        if let Some(instance) = &self._instance {
            while instance.activation.try_recv().is_ok() {
                activations += 1;
                if activations > 16 {
                    break;
                }
            }
        }
        if activations > 0 {
            self.show_window(ctx);
            self.log_info("收到另一个实例的激活请求，已显示主窗口");
        }

        if let Some(tray) = &self.tray {
            tray.update_state(self.monitor_on, self.busy, self.creds_ready());
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.allow_exit {
            self.engine.stop_monitor();
            return;
        }
        if self.config.close_to_tray && self.tray.is_some() && self.tray_host_available {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            self.log_info("已最小化到系统托盘；可在托盘中重新打开或退出");
        } else {
            self.allow_exit = true;
            self.engine.stop_monitor();
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_exit = true;
        self.engine.stop_monitor();
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot_path.clone() else {
            return;
        };
        if !self.screenshot_requested {
            self.screenshot_frames += 1;
            if self.screenshot_frames >= 6 {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(Default::default()));
                self.screenshot_requested = true;
            }
            return;
        }

        let events = ctx.input(|input| input.events.clone());
        for event in events {
            if let egui::Event::Screenshot { image, .. } = event {
                match save_screenshot(&path, &image) {
                    Ok(()) => self.log_success(format!("截图已保存：{path}")),
                    Err(err) => self.log_error(format!("保存截图失败：{err}")),
                }
                self.allow_exit = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
        }
    }

    fn first_update(&mut self, ctx: &egui::Context) {
        if self.started {
            return;
        }
        self.started = true;

        if self.minimized_start {
            if self.tray.is_some() && self.tray_host_available {
                // eframe 的 glow 后端会在首帧 post_rendering 中把窗口设为可见，
                // 这里在首帧末尾再发一次隐藏命令覆盖它。
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            } else {
                self.show_window(ctx);
                self.log_warn("启动时要求最小化，但系统托盘不可用，已直接显示窗口");
            }
        }

        match browser::resolve(&self.config) {
            Ok(resolved) => {
                self.browser_info = Some(resolved.description());
                self.log_info(format!("浏览器：{}", resolved.description()));
            }
            Err(err) => {
                self.log_warn(format!("浏览器检测：{err}"));
            }
        }

        if self.config.auto_start_monitor {
            if self.creds_ready() {
                self.start_monitor();
            } else {
                self.tab = Tab::Settings;
                self.notice = Some("请先在“设置”中填写学号与密码".to_string());
                if self.minimized_start {
                    self.show_window(ctx);
                }
            }
        }
    }

    fn is_settings_dirty(&self) -> bool {
        let s = &self.settings;
        s.student_id.trim() != self.config.student_id
            || s.password != self.password
            || s.poll_interval != self.config.poll_interval
            || s.browser_mode != self.config.browser_mode
            || s.custom_browser_path.trim() != self.config.custom_browser_path
            || s.close_to_tray != self.config.close_to_tray
            || s.auto_start_monitor != self.config.auto_start_monitor
            || s.autostart != self.config.autostart
    }

    fn save_settings(&mut self) -> bool {
        let student_id = self.settings.student_id.trim().to_string();
        if student_id.is_empty() {
            self.notice = Some("学号不能为空".to_string());
            return false;
        }
        if self.settings.password.is_empty() {
            self.notice = Some("密码不能为空".to_string());
            return false;
        }

        let password_changed = self.settings.password != self.password;
        if password_changed && let Err(err) = credentials::save_password(&self.settings.password) {
            self.notice = Some(format!("保存密码到系统密钥环失败：{err:#}"));
            self.log_error(format!("保存密码失败：{err:#}"));
            return false;
        }

        let mut cfg = self.config.clone();
        cfg.student_id = student_id;
        cfg.poll_interval = self
            .settings
            .poll_interval
            .clamp(MIN_POLL_INTERVAL, MAX_POLL_INTERVAL);
        cfg.browser_mode = self.settings.browser_mode;
        cfg.custom_browser_path = self.settings.custom_browser_path.trim().to_string();
        cfg.close_to_tray = self.settings.close_to_tray;
        cfg.auto_start_monitor = self.settings.auto_start_monitor;
        cfg.autostart = self.settings.autostart;

        if cfg.autostart != autostart::is_enabled() {
            if let Err(err) = autostart::set_enabled(cfg.autostart) {
                self.log_warn(err);
            } else if cfg.autostart {
                self.log_success("已启用开机自启（启动时最小化到托盘）");
            } else {
                self.log_info("已关闭开机自启");
            }
        }

        if let Err(err) = cfg.save() {
            self.notice = Some(format!("保存配置失败：{err:#}"));
            self.log_error(format!("保存配置失败：{err:#}"));
            return false;
        }

        self.config = cfg;
        self.password = self.settings.password.clone();
        self.settings.student_id = self.config.student_id.clone();
        self.settings.poll_interval = self.config.poll_interval;
        self.settings.custom_browser_path = self.config.custom_browser_path.clone();
        self.notice = None;
        self.log_success("设置已保存");

        match browser::resolve(&self.config) {
            Ok(resolved) => {
                self.browser_info = Some(resolved.description());
                self.log_info(format!("浏览器：{}", resolved.description()));
            }
            Err(err) => {
                self.browser_info = None;
                self.log_warn(format!("浏览器检测：{err}"));
            }
        }
        true
    }

    /// 操作前确保设置已保存、凭据可用。
    fn prepare_for_action(&mut self) -> bool {
        if self.is_settings_dirty() && !self.save_settings() {
            return false;
        }
        if !self.creds_ready() {
            self.tab = Tab::Settings;
            self.notice = Some("请先填写并保存学号与密码".to_string());
            return false;
        }
        true
    }

    fn start_auth_once(&mut self) -> bool {
        if !self.prepare_for_action() {
            return false;
        }
        let creds = Arc::new(Credentials {
            student_id: self.config.student_id.clone(),
            password: self.password.clone(),
        });
        match self.engine.auth_once(creds, self.config.clone()) {
            Ok(()) => {
                self.notice = None;
                true
            }
            Err(err) => {
                self.notice = Some(err.clone());
                self.log_warn(err);
                false
            }
        }
    }

    fn start_monitor(&mut self) -> bool {
        if !self.prepare_for_action() {
            return false;
        }
        let creds = Arc::new(Credentials {
            student_id: self.config.student_id.clone(),
            password: self.password.clone(),
        });
        match self.engine.start_monitor(creds, self.config.clone()) {
            Ok(()) => {
                self.notice = None;
                true
            }
            Err(err) => {
                self.notice = Some(err.clone());
                self.log_warn(err);
                false
            }
        }
    }

    fn stop_monitor(&mut self) {
        self.engine.stop_monitor();
    }

    fn clear_credentials(&mut self) {
        self.engine.stop_monitor();
        self.engine.cancel_auth();
        if let Err(err) = credentials::delete_password() {
            self.log_warn(format!("删除系统密钥环中的密码失败：{err:#}"));
        }
        self.password.clear();
        self.settings.password.clear();
        self.settings.student_id.clear();
        self.config.student_id.clear();
        if let Err(err) = self.config.save() {
            self.log_error(format!("保存配置失败：{err:#}"));
        }
        self.log_success("已清除已保存的账号密码");
        self.confirm_clear = false;
    }

    fn test_browser(&mut self) {
        let mut cfg = self.config.clone();
        cfg.browser_mode = self.settings.browser_mode;
        cfg.custom_browser_path = self.settings.custom_browser_path.trim().to_string();
        match browser::resolve(&cfg) {
            Ok(resolved) => {
                self.browser_info = Some(resolved.description());
                self.log_success(format!("浏览器可用：{}", resolved.description()));
            }
            Err(err) => {
                self.browser_info = None;
                self.log_error(format!("浏览器不可用：{err}"));
            }
        }
    }

    fn draw(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        egui::Panel::top("header")
            .exact_size(58.0)
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(4.0);
                    ui.heading("校园宽带自动认证");
                    ui.label(
                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .weak()
                            .small(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        status_chip(ui, self.phase);
                        if self.monitor_on {
                            chip(ui, "监控中", Color32::from_rgb(0x3b, 0x82, 0xf6));
                        }
                        if self.busy && !self.monitor_on {
                            ui.spinner();
                        }
                    });
                });
            });

        egui::Panel::bottom("footer").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "最后检测：{}",
                        self.last_check.as_deref().unwrap_or("--")
                    ))
                    .weak()
                    .small(),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "浏览器：{}",
                        self.browser_info.as_deref().unwrap_or("未检测")
                    ))
                    .weak()
                    .small(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("配置：{}", config::config_path().display()))
                            .weak()
                            .small(),
                    );
                });
            });
        });

        egui::Panel::left("controls")
            .resizable(false)
            .exact_size(376.0)
            .show(root, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Control, "控制");
                    ui.selectable_value(&mut self.tab, Tab::Settings, "设置");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.is_settings_dirty() {
                            ui.label(
                                RichText::new("有未保存的修改")
                                    .color(Color32::from_rgb(0xf5, 0xa6, 0x23))
                                    .small(),
                            );
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.tab {
                        Tab::Control => self.draw_control(ui),
                        Tab::Settings => self.draw_settings(ui),
                    });
            });

        egui::CentralPanel::default().show(root, |ui| self.draw_logs(ui));

        if let Some(message) = self.fatal.clone() {
            egui::Window::new("需要处理")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new(message).color(Color32::from_rgb(0xe5, 0x4b, 0x4b)));
                    ui.add_space(8.0);
                    if ui.button("知道了").clicked() {
                        self.fatal = None;
                    }
                });
        }

        if self.confirm_clear {
            egui::Window::new("清除账号密码")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label("将从系统密钥环和配置中删除已保存的账号密码，确定继续？");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("取消").clicked() {
                            self.confirm_clear = false;
                        }
                        if ui
                            .button(
                                RichText::new("清除").color(Color32::from_rgb(0xe5, 0x4b, 0x4b)),
                            )
                            .clicked()
                        {
                            self.clear_credentials();
                        }
                    });
                });
        }
    }

    fn draw_notice(&mut self, ui: &mut egui::Ui) {
        if let Some(notice) = self.notice.clone() {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(0xf5, 0xa6, 0x23, 28))
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(notice).color(Color32::from_rgb(0xd9, 0x8a, 0x0b)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("×").clicked() {
                                self.notice = None;
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }
    }

    fn draw_control(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let color = phase_color(self.phase);
        ui.label(
            RichText::new(self.phase.label())
                .size(30.0)
                .strong()
                .color(color),
        );
        let subtitle = if self.monitor_on {
            if self.busy {
                "监控运行中 · 正在认证"
            } else {
                "监控运行中 · 断线自动重连"
            }
        } else if self.busy {
            "正在执行认证任务"
        } else {
            "监控已停止"
        };
        ui.label(RichText::new(subtitle).weak());
        ui.add_space(10.0);

        self.draw_notice(ui);

        egui::Frame::group(ui.style())
            .inner_margin(10)
            .show(ui, |ui| {
                egui::Grid::new("account_info")
                    .num_columns(2)
                    .spacing([14.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("账号").weak());
                        if self.config.student_id.is_empty() {
                            ui.label(
                                RichText::new("未设置").color(Color32::from_rgb(0xe5, 0x4b, 0x4b)),
                            );
                        } else {
                            let masked = Credentials {
                                student_id: self.config.student_id.clone(),
                                password: String::new(),
                            }
                            .masked_student_id();
                            ui.label(masked);
                        }
                        ui.end_row();

                        ui.label(RichText::new("密码").weak());
                        if self.password.is_empty() {
                            ui.label(
                                RichText::new("未保存").color(Color32::from_rgb(0xe5, 0x4b, 0x4b)),
                            );
                        } else {
                            ui.label(
                                RichText::new("已保存到系统密钥环")
                                    .color(Color32::from_rgb(0x2f, 0x9e, 0x44)),
                            );
                        }
                        ui.end_row();

                        ui.label(RichText::new("检测间隔").weak());
                        ui.label(format!("{} 秒", self.config.poll_interval));
                        ui.end_row();

                        ui.label(RichText::new("WAN IP").weak());
                        ui.label(self.wan_ip.as_deref().unwrap_or("--"));
                        ui.end_row();

                        ui.label(RichText::new("最后检测").weak());
                        ui.label(self.last_check.as_deref().unwrap_or("--"));
                        ui.end_row();
                    });
            });

        ui.add_space(12.0);
        let can_auth = self.creds_ready() && !self.busy && !self.monitor_on;
        let can_start = self.creds_ready() && !self.busy && !self.monitor_on;
        let width = ui.available_width();

        if ui
            .add_enabled(
                can_auth,
                egui::Button::new(RichText::new("立即认证").strong())
                    .min_size(egui::vec2(width, 36.0)),
            )
            .clicked()
        {
            self.start_auth_once();
        }
        ui.add_space(4.0);
        if ui
            .add_enabled(
                can_start,
                egui::Button::new("开始监控").min_size(egui::vec2(width, 36.0)),
            )
            .clicked()
        {
            self.start_monitor();
        }
        ui.add_space(4.0);
        if ui
            .add_enabled(
                self.monitor_on,
                egui::Button::new("停止监控").min_size(egui::vec2(width, 36.0)),
            )
            .clicked()
        {
            self.stop_monitor();
        }

        if self.busy {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("任务进行中 ...").weak());
            });
            if !self.monitor_on
                && ui
                    .add_sized([width, 30.0], egui::Button::new("取消认证"))
                    .clicked()
            {
                self.engine.cancel_auth();
            }
        }

        if !self.creds_ready() {
            ui.add_space(10.0);
            ui.label(
                RichText::new("首次使用请先在“设置”中填写账号密码")
                    .color(Color32::from_rgb(0xf5, 0xa6, 0x23)),
            );
            if ui.button("去设置").clicked() {
                self.tab = Tab::Settings;
            }
        }

        ui.add_space(14.0);
        ui.label(RichText::new("说明").strong().small());
        ui.label(
            RichText::new(
                "在线判断只向黑洞地址 192.0.2.1 发探测请求，不会给任何外部服务器增加压力。\n\
                 检测周期约为「间隔 + 2 秒」；断线后自动重走认证流程。",
            )
            .weak()
            .small(),
        );
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        self.draw_notice(ui);
        ui.label(RichText::new("账号密码").strong());
        ui.label(
            RichText::new("密码保存到操作系统密钥环（Windows 凭据管理器 / Linux Secret Service），不写入配置文件。")
                .weak()
                .small(),
        );
        ui.add_space(6.0);

        ui.label("学号 / 宽带账号");
        ui.add_sized(
            [ui.available_width(), 28.0],
            egui::TextEdit::singleline(&mut self.settings.student_id),
        );
        ui.add_space(6.0);
        ui.label("密码");
        ui.add_sized(
            [ui.available_width(), 28.0],
            egui::TextEdit::singleline(&mut self.settings.password).password(true),
        );

        ui.add_space(12.0);
        ui.label(RichText::new("运行参数").strong());
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("断线检测间隔");
            ui.add(
                egui::DragValue::new(&mut self.settings.poll_interval)
                    .range(MIN_POLL_INTERVAL..=MAX_POLL_INTERVAL)
                    .suffix(" 秒"),
            );
        });
        ui.label(
            RichText::new("最小 3 秒。实际检测周期约为间隔 + 2 秒。")
                .weak()
                .small(),
        );

        ui.add_space(8.0);
        ui.checkbox(&mut self.settings.auto_start_monitor, "启动后自动开始监控");
        ui.checkbox(&mut self.settings.close_to_tray, "关闭窗口时最小化到托盘");
        ui.checkbox(&mut self.settings.autostart, "开机自启（最小化到托盘）");

        ui.add_space(12.0);
        ui.label(RichText::new("认证浏览器").strong());
        ui.horizontal(|ui| {
            ui.label("来源");
            egui::ComboBox::from_id_salt("browser_mode")
                .selected_text(self.settings.browser_mode.label())
                .width(180.0)
                .show_ui(ui, |ui| {
                    for mode in BrowserMode::ALL {
                        ui.selectable_value(&mut self.settings.browser_mode, mode, mode.label());
                    }
                });
        });
        if self.settings.browser_mode == BrowserMode::Custom {
            ui.label("可执行文件路径");
            ui.add_sized(
                [ui.available_width(), 28.0],
                egui::TextEdit::singleline(&mut self.settings.custom_browser_path),
            );
        }
        ui.label(
            RichText::new(
                "认证必须使用真实 Chromium 内核（系统 Edge/Chrome/Chromium，或程序目录下的随包 Chromium）。",
            )
            .weak()
            .small(),
        );
        if ui.button("检测浏览器").clicked() {
            self.test_browser();
        }

        ui.add_space(14.0);
        let width = ui.available_width();
        if ui
            .add_sized(
                [width, 34.0],
                egui::Button::new(RichText::new("保存设置").strong()),
            )
            .clicked()
        {
            self.save_settings();
        }
        ui.add_space(4.0);
        if ui
            .add_sized(
                [width, 30.0],
                egui::Button::new(
                    RichText::new("清除已保存的账号密码")
                        .color(Color32::from_rgb(0xe5, 0x4b, 0x4b)),
                ),
            )
            .clicked()
        {
            self.confirm_clear = true;
        }

        ui.add_space(12.0);
        ui.label(
            RichText::new("修改检测间隔或浏览器设置后，需要重新开始监控才会生效。")
                .weak()
                .small(),
        );
    }

    fn draw_logs(&mut self, ui: &mut egui::Ui) {
        let mut clear = false;
        let mut auto_scroll = self.auto_scroll;
        ui.horizontal(|ui| {
            ui.heading("运行日志");
            ui.label(
                RichText::new(format!("{} 条", self.logs.len()))
                    .weak()
                    .small(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("清空").clicked() {
                    clear = true;
                }
                ui.checkbox(&mut auto_scroll, "自动滚动");
            });
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(auto_scroll)
            .show(ui, |ui| {
                for line in &self.logs {
                    let color = match line.level {
                        LogLevel::Info => Color32::from_rgb(0xd0, 0xd4, 0xda),
                        LogLevel::Success => Color32::from_rgb(0x4c, 0xc3, 0x6a),
                        LogLevel::Warn => Color32::from_rgb(0xf5, 0xc2, 0x42),
                        LogLevel::Error => Color32::from_rgb(0xf0, 0x6b, 0x6b),
                    };
                    ui.label(
                        RichText::new(format!("[{}] {}", line.time, line.text))
                            .monospace()
                            .size(13.0)
                            .color(color),
                    );
                }
            });

        self.auto_scroll = auto_scroll;
        if clear {
            self.logs.clear();
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_tray(ctx);
        self.drain_events();
        if self.pending_show_window {
            self.pending_show_window = false;
            self.show_window(ctx);
        }
        self.drain_tray(ctx);
        self.first_update(ctx);
        self.handle_screenshot(ctx);
        self.handle_close_request(ctx);

        // 后台任务通过 mpsc 回传事件，定期唤醒界面处理日志与托盘/激活请求。
        let delay = if self.busy || self.monitor_on {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        };
        ctx.request_repaint_after(delay);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.engine.stop_monitor();
    }
}

fn phase_color(phase: Phase) -> Color32 {
    match phase {
        Phase::Idle => Color32::from_rgb(0x9c, 0xa3, 0xaf),
        Phase::Probing => Color32::from_rgb(0x60, 0xa5, 0xfa),
        Phase::Online => Color32::from_rgb(0x4c, 0xc3, 0x6a),
        Phase::Offline => Color32::from_rgb(0xf5, 0xa6, 0x23),
        Phase::Authenticating => Color32::from_rgb(0xf5, 0xc2, 0x42),
        Phase::Error => Color32::from_rgb(0xf0, 0x6b, 0x6b),
    }
}

fn status_chip(ui: &mut egui::Ui, phase: Phase) {
    chip(ui, phase.label(), phase_color(phase));
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            32,
        ))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).strong());
        });
}

/// 加载系统 CJK 字体，否则中文会显示为方块。
fn setup_fonts(ctx: &egui::Context) -> bool {
    let Some((bytes, index)) = load_cjk_font() else {
        return false;
    };
    let mut fonts = egui::FontDefinitions::default();
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    fonts.font_data.insert("cjk".to_owned(), Arc::new(data));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    true
}

fn load_cjk_font() -> Option<(Vec<u8>, u32)> {
    #[cfg(target_os = "linux")]
    let candidates: Vec<(String, u32)> = {
        let mut list = vec![
            (
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".to_string(),
                2, // Noto Sans CJK SC
            ),
            (
                "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc".to_string(),
                2,
            ),
            (
                "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc".to_string(),
                2,
            ),
            (
                "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf".to_string(),
                0,
            ),
            (
                "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf".to_string(),
                0,
            ),
            (
                "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc".to_string(),
                0,
            ),
            (
                "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc".to_string(),
                0,
            ),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            let home = std::path::PathBuf::from(home);
            list.push((
                home.join(".local/share/fonts/NotoSansCJK-Regular.ttc")
                    .to_string_lossy()
                    .into_owned(),
                2,
            ));
            list.push((
                home.join(".fonts/NotoSansCJK-Regular.ttc")
                    .to_string_lossy()
                    .into_owned(),
                2,
            ));
        }
        list
    };

    #[cfg(target_os = "windows")]
    let candidates: Vec<(String, u32)> = {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string());
        vec![
            (format!("{windir}\\Fonts\\msyh.ttc"), 0),
            (format!("{windir}\\Fonts\\msyhbd.ttc"), 0),
            (format!("{windir}\\Fonts\\simhei.ttf"), 0),
            (format!("{windir}\\Fonts\\simsun.ttc"), 0),
        ]
    };

    #[cfg(target_os = "macos")]
    let candidates: Vec<(String, u32)> = vec![
        ("/System/Library/Fonts/PingFang.ttc".to_string(), 0),
        ("/System/Library/Fonts/Hiragino Sans GB.ttc".to_string(), 0),
    ];

    for (path, index) in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            return Some((bytes, index));
        }
    }

    // 兜底：交给 fontconfig 找支持中文的字体（覆盖 Arch/Fedora 等路径差异）
    #[cfg(target_os = "linux")]
    if let Some(found) = fontconfig_cjk_font() {
        return Some(found);
    }
    None
}

#[cfg(target_os = "linux")]
fn fontconfig_cjk_font() -> Option<(Vec<u8>, u32)> {
    use std::process::Command;
    let output = Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}", "sans-serif:lang=zh-cn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let path = lines.next()?.trim();
    let index = lines.next()?.trim().parse::<u32>().unwrap_or(0);
    if path.is_empty() {
        return None;
    }
    std::fs::read(path).ok().map(|bytes| (bytes, index))
}

fn save_screenshot(path: &str, image: &egui::ColorImage) -> std::io::Result<()> {
    let [width, height] = image.size;
    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&pixel.to_array());
    }
    let buffer = ::image::RgbaImage::from_raw(width as u32, height as u32, rgba)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "截图尺寸不匹配"))?;
    buffer
        .save(path)
        .map_err(|err| std::io::Error::other(format!("{err}")))
}
