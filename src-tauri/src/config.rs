//! 应用配置：账号、检测间隔、浏览器来源、界面行为。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_POLL_INTERVAL: u64 = 5;
pub const MIN_POLL_INTERVAL: u64 = 3;
pub const MAX_POLL_INTERVAL: u64 = 3600;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserMode {
    /// 优先使用随包浏览器，其次系统浏览器
    #[default]
    Auto,
    /// 仅使用系统 Edge/Chrome/Chromium
    System,
    /// 仅使用程序目录下的随包 Chromium
    Bundled,
    /// 使用用户指定的可执行文件
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub student_id: String,
    pub poll_interval: u64,
    pub browser_mode: BrowserMode,
    pub custom_browser_path: String,
    /// 点击窗口关闭按钮时隐藏到托盘而不是退出
    pub close_to_tray: bool,
    /// 程序启动后自动进入断线监控
    pub auto_start_monitor: bool,
    /// 开机自启（持久化的是期望状态，实际以系统为准）
    pub autostart: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            student_id: String::new(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            browser_mode: BrowserMode::Auto,
            custom_browser_path: String::new(),
            close_to_tray: true,
            auto_start_monitor: true,
            autostart: false,
        }
    }
}

impl AppConfig {
    /// 读取配置。文件缺失时返回默认值；文件损坏时返回默认值并附带提示。
    pub fn load() -> (Self, Option<String>) {
        let path = config_path();
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(err) => {
                return (
                    Self::default(),
                    Some(format!("读取配置失败（{err}），已使用默认设置")),
                );
            }
        };
        match serde_json::from_str::<Self>(&raw) {
            Ok(mut cfg) => {
                cfg.normalize();
                (cfg, None)
            }
            Err(err) => (
                Self::default(),
                Some(format!("配置文件格式错误（{err}），已使用默认设置")),
            ),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败：{}", parent.display()))?;
        }
        let mut cfg = self.clone();
        cfg.normalize();
        let json = serde_json::to_string_pretty(&cfg)?;
        // 先写临时文件再重命名，避免断电/崩溃留下半截配置
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).with_context(|| format!("写入配置失败：{}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("保存配置失败：{}", path.display()))?;
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.student_id = self.student_id.trim().to_string();
        self.poll_interval = self
            .poll_interval
            .clamp(MIN_POLL_INTERVAL, MAX_POLL_INTERVAL);
        self.custom_browser_path = self.custom_browser_path.trim().to_string();
    }
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// 配置目录：Windows `%APPDATA%\cmcc-campus-auth`，Linux `$XDG_CONFIG_HOME/cmcc-campus-auth`
/// （默认 `~/.config/cmcc-campus-auth`），macOS `~/Library/Application Support/cmcc-campus-auth`。
#[cfg(target_os = "windows")]
pub fn config_dir() -> PathBuf {
    env_path("APPDATA")
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cmcc-campus-auth")
}

#[cfg(target_os = "windows")]
pub fn data_dir() -> PathBuf {
    env_path("LOCALAPPDATA")
        .or_else(|| env_path("APPDATA"))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cmcc-campus-auth")
}

#[cfg(target_os = "macos")]
pub fn config_dir() -> PathBuf {
    env_path("HOME")
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support/cmcc-campus-auth")
}

#[cfg(target_os = "macos")]
pub fn data_dir() -> PathBuf {
    config_dir()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn config_dir() -> PathBuf {
    env_path("XDG_CONFIG_HOME")
        .or_else(|| env_path("HOME").map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cmcc-campus-auth")
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn data_dir() -> PathBuf {
    env_path("XDG_DATA_HOME")
        .or_else(|| env_path("HOME").map(|home| home.join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cmcc-campus-auth")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_clamps_interval() {
        let mut cfg = AppConfig {
            poll_interval: 1,
            student_id: "  123  ".into(),
            ..Default::default()
        };
        cfg.normalize();
        assert_eq!(cfg.poll_interval, MIN_POLL_INTERVAL);
        assert_eq!(cfg.student_id, "123");

        cfg.poll_interval = 99_999;
        cfg.normalize();
        assert_eq!(cfg.poll_interval, MAX_POLL_INTERVAL);
    }

    #[test]
    fn browser_mode_roundtrip() {
        let cfg = AppConfig {
            browser_mode: BrowserMode::Bundled,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.browser_mode, BrowserMode::Bundled);
    }
}
