#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};

use tauri::Manager;

use cmcc_campus_auth::config::AppConfig;
use cmcc_campus_auth::engine::Engine;
use cmcc_campus_auth::events::EventSink;
use cmcc_campus_auth::state::{AppState, LogStore, Snapshot};
use cmcc_campus_auth::{
    autostart, bridge, browser, commands, config, credentials, logging, tray, window,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("cmcc-campus-auth {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "校园宽带自动认证（Tauri 版）\n\n\
             用法：cmcc-campus-auth [--minimized]\n\n\
             --minimized   启动后最小化到系统托盘（开机自启用）"
        );
        return;
    }

    logging::init();
    let minimized = args.iter().any(|arg| arg == "--minimized");
    if let Err(err) = run(minimized) {
        logging::write_line("ERROR", &format!("启动失败：{err}"));
        eprintln!("启动失败：{err}");
        std::process::exit(1);
    }
}

fn run(minimized: bool) -> Result<(), Box<dyn std::error::Error>> {
    let app = tauri::Builder::default()
        // 单实例插件必须最先注册
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(state) = app.try_state::<AppState>() {
                state.log_info("收到另一个实例的激活请求，已显示主窗口");
            }
            window::show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::attach_events,
            commands::ui_ready,
            commands::save_settings,
            commands::auth_now,
            commands::start_monitor,
            commands::stop_monitor,
            commands::cancel_auth,
            commands::test_browser,
            commands::clear_credentials,
            commands::dismiss_fatal,
            commands::pick_browser_path,
        ])
        .setup(move |app| {
            // 初始化放在 setup 里：单实例插件在 build 阶段就会让第二个实例退出，
            // 因此重复启动不会再去读系统密钥环（避免弹出解锁提示）。
            let (config, config_warning) = AppConfig::load();
            let mut notices = Vec::new();
            if let Some(warning) = config_warning {
                logging::write_line("WARN", &warning);
                notices.push(warning);
            }

            let password = match credentials::load_password() {
                Ok(Some(password)) if !password.is_empty() => Some(password),
                Ok(_) => None,
                Err(err) => {
                    let warning = format!("读取系统密钥环失败：{err:#}");
                    logging::write_line("WARN", &warning);
                    notices.push(warning);
                    None
                }
            };

            let mut config = config;
            let autostart_actual = autostart::is_enabled();
            config.autostart = autostart_actual;

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            let (tx, rx) = mpsc::channel();
            let sink = EventSink::new(tx);
            let engine = Engine::new(runtime.handle().clone(), sink.clone());

            app.manage(AppState {
                engine: Mutex::new(engine),
                sink: Mutex::new(sink),
                config: Mutex::new(config),
                password: Mutex::new(password),
                snapshot: Mutex::new(Snapshot::default()),
                logs: Mutex::new(LogStore::default()),
                channel: Mutex::new(None),
                tray: Mutex::new(None),
                tray_ok: AtomicBool::new(false),
                allow_exit: AtomicBool::new(false),
                ui_ready: AtomicBool::new(false),
                minimized_start: minimized,
                rx: Mutex::new(Some(rx)),
                _runtime: runtime,
            });

            let handle = app.handle().clone();
            if let Ok(resource_dir) = app.path().resource_dir() {
                browser::set_resource_dir(resource_dir);
            }

            let state = app.state::<AppState>();

            state.log_info(format!(
                "校园宽带自动认证 v{} 启动",
                env!("CARGO_PKG_VERSION")
            ));
            state.log_info(format!("配置目录：{}", config::config_dir().display()));
            for warning in notices {
                state.log_warn(warning);
            }

            let (student_id, has_password) = {
                let student_id = state
                    .config
                    .lock()
                    .map(|cfg| cfg.student_id.clone())
                    .unwrap_or_default();
                let has_password = state.password.lock().is_ok_and(|pwd| pwd.is_some());
                (student_id, has_password)
            };
            if !student_id.is_empty() {
                state.log_info(format!("已加载账号：{student_id}"));
            }
            if has_password {
                state.log_info("已从系统密钥环加载密码");
            } else {
                state.log_info("尚未保存密码，请在“设置”中填写账号密码");
            }

            // 系统托盘
            let host_available = tray::host_available();
            if !host_available {
                state.log_warn(
                    "未检测到系统托盘宿主（GNOME 请启用 AppIndicator/Ubuntu AppIndicators 扩展）；\
                     将禁用“关闭到托盘”，关闭窗口会直接退出",
                );
            }
            match tray::create(&handle) {
                Ok(handles) => {
                    if let Ok(mut slot) = state.tray.lock() {
                        *slot = Some(handles);
                    }
                    state.tray_ok.store(host_available, Ordering::Relaxed);
                    if host_available {
                        state.log_success("系统托盘已启用");
                    } else {
                        state.log_warn("托盘服务已创建，但图标可能不可见");
                    }
                    // 按当前凭据状态初始化菜单项，避免无凭据时可点击
                    tray::update_menu_state(&handle);
                }
                Err(err) => {
                    state.log_warn(format!("{err}；关闭窗口将直接退出"));
                }
            }

            // 事件转发线程
            let receiver = state.rx.lock().ok().and_then(|mut slot| slot.take());
            if let Some(receiver) = receiver {
                bridge::spawn(handle.clone(), receiver);
            }

            // 启动时检测浏览器，便于在日志里尽早暴露配置问题
            let cfg = state
                .config
                .lock()
                .map(|cfg| cfg.clone())
                .unwrap_or_default();
            match browser::resolve(&cfg) {
                Ok(resolved) => {
                    if let Ok(mut snapshot) = state.snapshot.lock() {
                        snapshot.browser = Some(resolved.description());
                    }
                    state.log_info(format!("浏览器：{}", resolved.description()));
                }
                Err(err) => state.log_warn(format!("浏览器检测：{err}")),
            }

            // 前端脚本异常时，3 秒后仍要显示窗口并保证后台监控可启动
            let fallback = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let state = fallback.state::<AppState>();
                if state.ui_ready.load(Ordering::Relaxed) {
                    return;
                }
                window::show_on_startup(&fallback);
                let auto_start = state
                    .config
                    .lock()
                    .map(|cfg| cfg.auto_start_monitor)
                    .unwrap_or(false);
                if auto_start && state.creds_ready() && !state.snapshot().monitoring {
                    let _ = commands::do_start_monitor(&fallback);
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let state = app.state::<AppState>();
                if state.allow_exit.load(Ordering::SeqCst) {
                    return;
                }
                let close_to_tray = state
                    .config
                    .lock()
                    .map(|cfg| cfg.close_to_tray)
                    .unwrap_or(false);
                if close_to_tray && state.tray_ok.load(Ordering::Relaxed) {
                    api.prevent_close();
                    // 不能在 CloseRequested 处理器里同步 hide：GTK 会在 delete-event
                    // 处理期间与 WM 帧窗口失去同步，之后 show() 无法重新映射。
                    // 推迟到事件循环下一轮再隐藏。
                    let window = window.clone();
                    let _ = app.run_on_main_thread(move || {
                        let _ = window.hide();
                    });
                    state.log_info("已最小化到系统托盘；可在托盘中重新打开或退出");
                } else {
                    state.allow_exit.store(true, Ordering::SeqCst);
                    if let Ok(engine) = state.engine.lock() {
                        engine.stop_monitor();
                    }
                    app.exit(0);
                }
            }
        })
        .build(tauri::generate_context!())?;

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            let state = app_handle.state::<AppState>();
            if let Ok(engine) = state.engine.lock() {
                engine.stop_monitor();
            }
        }
    });

    Ok(())
}
