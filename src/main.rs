#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod auth;
mod autostart;
mod browser;
mod config;
mod credentials;
mod engine;
mod events;
mod icon;
mod logging;
mod ocr;
mod probe;
mod single_instance;
mod tray;

use app::App;
use config::AppConfig;
use engine::Engine;
use events::EventSink;
use single_instance::Acquire;

fn main() {
    if let Err(err) = run() {
        logging::write_line("ERROR", &format!("启动失败：{err}"));
        eprintln!("启动失败：{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("cmcc-campus-auth {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "校园宽带自动认证（Rust GUI 版）\n\n\
             用法：cmcc-campus-auth [--minimized]\n\n\
             --minimized   启动后最小化到系统托盘（开机自启用）\n\
             --screenshot <路径>  渲染后截图并退出（调试用）"
        );
        return Ok(());
    }
    logging::init();
    let minimized = args.iter().any(|arg| arg == "--minimized");
    let screenshot_path = args
        .iter()
        .position(|arg| arg == "--screenshot")
        .and_then(|index| args.get(index + 1))
        .cloned();

    let instance = match single_instance::acquire() {
        Ok(Acquire::First(guard)) => Some(guard),
        Ok(Acquire::AlreadyRunning(notified)) => {
            if notified {
                println!("程序已在运行，已通知现有实例显示窗口");
            } else {
                println!("程序已在运行");
            }
            return Ok(());
        }
        Err(err) => {
            eprintln!("警告：单实例锁不可用（{err}），继续启动");
            None
        }
    };

    let (config, config_warning) = AppConfig::load();
    let mut warnings = Vec::new();
    if let Some(warning) = config_warning {
        logging::write_line("WARN", &warning);
        warnings.push(warning);
    }
    let password = match credentials::load_password() {
        Ok(Some(password)) => password,
        Ok(None) => String::new(),
        Err(err) => {
            let warning = format!("读取系统密钥环失败：{err:#}");
            logging::write_line("WARN", &warning);
            warnings.push(warning);
            String::new()
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let engine = Engine::new(runtime.handle().clone(), EventSink::new(tx));

    let icon = eframe::egui::IconData {
        rgba: icon::icon_rgba(64),
        width: 64,
        height: 64,
    };
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title("校园宽带自动认证")
        .with_inner_size([1024.0, 700.0])
        .with_min_inner_size([840.0, 560.0])
        .with_icon(icon)
        .with_visible(!minimized);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "校园宽带自动认证",
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                cc,
                config,
                password,
                warnings,
                engine,
                rx,
                instance,
                minimized,
                screenshot_path,
            )))
        }),
    )?;

    drop(runtime);
    Ok(())
}
