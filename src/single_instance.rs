//! 单实例控制。
//!
//! 通过配置文件目录里的锁文件保证只运行一个实例；后启动的实例会尝试通过
//! 本机回环端口通知已运行的实例把窗口显示出来，然后退出。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use crate::config;

pub enum Acquire {
    /// 当前进程是唯一实例
    First(InstanceGuard),
    /// 已有实例在运行；false 表示没能通知到它
    AlreadyRunning(bool),
}

pub struct InstanceGuard {
    _lock: File,
    pub activation: Receiver<()>,
    port_file: PathBuf,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.port_file);
        let _ = self._lock.unlock();
    }
}

pub fn acquire() -> Result<Acquire, String> {
    let dir = config::config_dir();
    fs::create_dir_all(&dir).map_err(|err| format!("创建配置目录失败：{err}"))?;
    let lock_path = dir.join("instance.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|err| format!("打开实例锁失败：{err}"))?;

    match lock.try_lock() {
        Ok(()) => {}
        Err(_) => {
            let notified = notify_existing(&dir);
            return Ok(Acquire::AlreadyRunning(notified));
        }
    }

    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|err| format!("监听激活端口失败：{err}"))?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("读取激活端口失败：{err}"))?
        .port();
    let port_file = dir.join("instance.port");
    fs::write(&port_file, port.to_string()).map_err(|err| format!("写入激活端口失败：{err}"))?;

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            // 防止本地进程连上后不发数据把监听线程永久卡住
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            // 先读掉客户端数据再关闭，否则内核可能回 RST 而不是 FIN
            let mut scratch = [0u8; 16];
            let _ = stream.read(&mut scratch);
            let _ = tx.send(());
        }
    });

    Ok(Acquire::First(InstanceGuard {
        _lock: lock,
        activation: rx,
        port_file,
    }))
}

fn notify_existing(dir: &std::path::Path) -> bool {
    let port_file = dir.join("instance.port");
    let Ok(text) = fs::read_to_string(&port_file) else {
        return false;
    };
    let Ok(port) = text.trim().parse::<u16>() else {
        return false;
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(1)) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    stream.write_all(b"SHOW\n").is_ok()
}
