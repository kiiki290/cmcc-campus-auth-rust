//! 核心线程与 GUI 之间的事件定义。

use std::sync::mpsc::Sender;

use chrono::Local;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Success,
    Warn,
    Error,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Success => "OK",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LogLine {
    pub time: String,
    pub level: LogLevel,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// 尚未检测 / 监控已停止
    Idle,
    /// 正在探测
    Probing,
    /// 已在线
    Online,
    /// 探测到断线
    Offline,
    /// 正在认证
    Authenticating,
    /// 出错（网络、浏览器、认证失败等）
    Error,
}

/// 认证流程阶段（仅供界面展示进度，不参与任何判断）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStage {
    /// 探测完成，已拿到 Portal 地址
    ProbeDone,
    /// 认证页面与登录表单已就绪
    PortalLoaded,
    /// 验证码识别完成
    CaptchaReady,
    /// 登录表单已提交
    Submitted,
    /// 正在等待 BRAS 放行
    Releasing,
}

#[derive(Clone, Debug)]
pub enum EngineEvent {
    Log(LogLine),
    Phase(Phase),
    /// 认证流程进度埋点（纯展示）
    Stage(AuthStage),
    MonitorActive(bool),
    Busy(bool),
    WanIp(Option<String>),
    /// 一轮探测完成（时间戳用于界面展示“最后检测”）
    Ticked(String),
    BrowserResolved(String),
    Fatal(String),
}

/// 线程安全的事件发送器。失败（界面已退出）时静默忽略。
#[derive(Clone)]
pub struct EventSink {
    tx: Sender<EngineEvent>,
}

impl EventSink {
    pub fn new(tx: Sender<EngineEvent>) -> Self {
        Self { tx }
    }

    pub fn send(&self, event: EngineEvent) {
        let _ = self.tx.send(event);
    }

    pub fn log(&self, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        crate::logging::write_line(level.label(), &text);
        self.send(EngineEvent::Log(LogLine {
            time: Local::now().format("%H:%M:%S").to_string(),
            level,
            text,
        }));
    }

    pub fn info(&self, text: impl Into<String>) {
        self.log(LogLevel::Info, text);
    }

    pub fn success(&self, text: impl Into<String>) {
        self.log(LogLevel::Success, text);
    }

    pub fn warn(&self, text: impl Into<String>) {
        self.log(LogLevel::Warn, text);
    }

    pub fn error(&self, text: impl Into<String>) {
        self.log(LogLevel::Error, text);
    }
}
