//! 追加式文件日志：Release 的 Windows 程序没有控制台，出问题时可让用户提供 app.log。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use chrono::Local;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

pub fn init() {
    let dir = crate::config::config_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("app.log");
    if fs::metadata(&path)
        .map(|m| m.len() > MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::rename(&path, dir.join("app.log.1"));
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = LOG_FILE.set(Mutex::new(file));
    write_line(
        "INFO",
        &format!("==== 启动 v{} ====", env!("CARGO_PKG_VERSION")),
    );
}

pub fn write_line(level: &str, text: &str) {
    let Some(file) = LOG_FILE.get() else {
        return;
    };
    if let Ok(mut file) = file.lock() {
        let _ = writeln!(
            file,
            "[{}] [{level}] {text}",
            Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }
}
