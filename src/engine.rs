//! 后台任务：单次认证、断线监控循环。
//!
//! GUI 线程只负责界面；所有网络/浏览器任务都跑在独立的 tokio 运行时里，
//! 通过 `EngineEvent` 通道回传状态与日志。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Local;

use crate::auth::{self, CancelFlag, Credentials, RetryOutcome, authenticate_with_retry};
use crate::config::AppConfig;
use crate::events::{EngineEvent, EventSink, Phase};
use crate::probe;

pub struct Engine {
    handle: tokio::runtime::Handle,
    sink: EventSink,
    monitor_cancel: Arc<Mutex<Option<CancelFlag>>>,
    auth_cancel: Arc<Mutex<Option<CancelFlag>>>,
    monitor_active: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
}

/// 任务返回/panic/被中止时都能正确复位界面状态，避免按钮永久卡在“进行中”。
struct BusyGuard {
    sink: EventSink,
    busy: Arc<AtomicBool>,
    auth_cancel: Arc<Mutex<Option<CancelFlag>>>,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        // 先清 slot 再发布 busy=false，避免 GUI 启动的新任务写入的 CancelFlag 被旧 guard 清掉
        if let Ok(mut slot) = self.auth_cancel.lock() {
            *slot = None;
        }
        if std::thread::panicking() {
            self.sink.error("✗ 认证任务异常退出");
            self.sink.send(EngineEvent::Phase(Phase::Error));
            self.sink
                .send(EngineEvent::Fatal("认证任务异常退出".to_string()));
        }
        self.sink.send(EngineEvent::Busy(false));
        self.busy.store(false, Ordering::SeqCst);
    }
}

struct MonitorGuard {
    sink: EventSink,
    active: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    cancel_slot: Arc<Mutex<Option<CancelFlag>>>,
}

impl Drop for MonitorGuard {
    fn drop(&mut self) {
        // 先清 slot 再发布 active=false，避免 GUI 启动的新监控写入的 CancelFlag 被旧 guard 清掉
        if let Ok(mut slot) = self.cancel_slot.lock() {
            *slot = None;
        }
        if std::thread::panicking() {
            self.sink.error("✗ 监控任务异常退出");
            self.sink.send(EngineEvent::Phase(Phase::Error));
            self.sink
                .send(EngineEvent::Fatal("监控任务异常退出".to_string()));
        }
        self.active.store(false, Ordering::SeqCst);
        self.sink.send(EngineEvent::MonitorActive(false));
        self.sink.send(EngineEvent::Busy(false));
        self.busy.store(false, Ordering::SeqCst);
    }
}

impl Engine {
    pub fn new(handle: tokio::runtime::Handle, sink: EventSink) -> Self {
        Self {
            handle,
            sink,
            monitor_cancel: Arc::new(Mutex::new(None)),
            auth_cancel: Arc::new(Mutex::new(None)),
            monitor_active: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_monitoring(&self) -> bool {
        self.monitor_active.load(Ordering::Relaxed)
    }

    /// 立即认证一次（含 3 轮重试）。
    pub fn auth_once(&self, creds: Arc<Credentials>, cfg: AppConfig) -> Result<(), String> {
        if self.is_monitoring() {
            return Err("正在监控中，请先停止监控".to_string());
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("已有认证任务在进行".to_string());
        }

        let cancel = CancelFlag::new();
        if let Ok(mut slot) = self.auth_cancel.lock() {
            *slot = Some(cancel.clone());
        }
        let sink = self.sink.clone();
        let busy = self.busy.clone();
        let auth_cancel = self.auth_cancel.clone();
        self.handle.spawn(async move {
            let _guard = BusyGuard {
                sink: sink.clone(),
                busy: busy.clone(),
                auth_cancel,
            };
            sink.send(EngineEvent::Busy(true));
            sink.send(EngineEvent::Phase(Phase::Probing));
            sink.info("开始单次认证 ...");
            let outcome = authenticate_with_retry(&creds, &cfg, &sink, &cancel).await;
            match outcome {
                RetryOutcome::Online => {
                    sink.send(EngineEvent::Phase(Phase::Online));
                    sink.success("✓ 当前已在线");
                }
                RetryOutcome::Authenticated => {
                    sink.send(EngineEvent::Phase(Phase::Online));
                    sink.success("✓ 认证流程完成");
                }
                RetryOutcome::Fatal(message) => {
                    sink.send(EngineEvent::Phase(Phase::Error));
                    sink.send(EngineEvent::Fatal(message));
                }
                RetryOutcome::Failed(message) => {
                    sink.send(EngineEvent::Phase(Phase::Error));
                    sink.error(format!("✗ 认证失败：{message}"));
                }
                RetryOutcome::Cancelled => {
                    sink.send(EngineEvent::Phase(Phase::Idle));
                    sink.info("已取消认证");
                }
            }
        });
        Ok(())
    }

    /// 开始断线监控。
    pub fn start_monitor(&self, creds: Arc<Credentials>, cfg: AppConfig) -> Result<(), String> {
        if self.is_monitoring() {
            return Err("监控已经在运行".to_string());
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("已有认证任务在进行".to_string());
        }

        let cancel = CancelFlag::new();
        if let Ok(mut slot) = self.monitor_cancel.lock() {
            *slot = Some(cancel.clone());
        }
        self.monitor_active.store(true, Ordering::SeqCst);

        let sink = self.sink.clone();
        let busy = self.busy.clone();
        let active = self.monitor_active.clone();
        let cancel_slot = self.monitor_cancel.clone();
        let interval = Duration::from_secs(cfg.poll_interval.max(1));

        sink.send(EngineEvent::MonitorActive(true));
        sink.send(EngineEvent::Busy(false));
        sink.send(EngineEvent::Phase(Phase::Probing));
        sink.success(format!(
            "进入断线监控模式（间隔 {}s，在线探测超时 {}s，检测周期约 {}s）",
            cfg.poll_interval,
            probe::PROBE_TIMEOUT.as_secs(),
            cfg.poll_interval + probe::PROBE_TIMEOUT.as_secs()
        ));

        self.handle.spawn(async move {
            let _guard = MonitorGuard {
                sink: sink.clone(),
                active: active.clone(),
                busy: busy.clone(),
                cancel_slot: cancel_slot.clone(),
            };
            let mut ended_with_error = false;
            let mut was_online: Option<bool> = None;
            loop {
                if cancel.is_cancelled() {
                    break;
                }

                sink.send(EngineEvent::Phase(Phase::Probing));
                let offline = match tokio::task::spawn_blocking(probe::is_offline_sync).await {
                    Ok(value) => value,
                    Err(err) => {
                        sink.warn(format!("探测任务异常：{err}"));
                        false
                    }
                };
                sink.send(EngineEvent::Ticked(
                    Local::now().format("%H:%M:%S").to_string(),
                ));

                if offline {
                    sink.send(EngineEvent::Phase(Phase::Offline));
                    sink.info("检测到断线，开始认证（最多 3 轮，间隔 5 秒）...");
                    sink.send(EngineEvent::Busy(true));
                    let outcome = authenticate_with_retry(&creds, &cfg, &sink, &cancel).await;
                    sink.send(EngineEvent::Busy(false));
                    match outcome {
                        RetryOutcome::Online | RetryOutcome::Authenticated => {
                            was_online = Some(true);
                            sink.send(EngineEvent::Phase(Phase::Online));
                            sink.success("✓ 重连成功，继续监控");
                        }
                        RetryOutcome::Fatal(message) => {
                            sink.send(EngineEvent::Phase(Phase::Error));
                            sink.error(format!("✗ 致命错误，停止监控：{message}"));
                            sink.send(EngineEvent::Fatal(message));
                            ended_with_error = true;
                            break;
                        }
                        RetryOutcome::Failed(message) => {
                            sink.send(EngineEvent::Phase(Phase::Error));
                            sink.error(format!(
                                "✗ 断线后 3 轮认证均失败（{message}）。30 秒后重新检测，监控继续运行"
                            ));
                            if !auth::sleep_cancellable(Duration::from_secs(30), &cancel).await {
                                break;
                            }
                            continue;
                        }
                        RetryOutcome::Cancelled => break,
                    }
                } else {
                    sink.send(EngineEvent::Phase(Phase::Online));
                    if was_online != Some(true) {
                        sink.info("在线 ✓");
                    }
                    was_online = Some(true);
                }

                if !auth::sleep_cancellable(interval, &cancel).await {
                    break;
                }
            }

            if !ended_with_error {
                sink.info("已停止监控");
                sink.send(EngineEvent::Phase(Phase::Idle));
            }
        });

        Ok(())
    }

    /// 请求停止监控（当前认证轮会先结束）。
    pub fn stop_monitor(&self) {
        if let Ok(slot) = self.monitor_cancel.lock()
            && let Some(cancel) = slot.as_ref()
        {
            cancel.cancel();
            self.sink.info("正在停止监控 ...");
        }
    }

    /// 取消正在进行的单次认证。
    pub fn cancel_auth(&self) {
        if let Ok(slot) = self.auth_cancel.lock()
            && let Some(cancel) = slot.as_ref()
        {
            cancel.cancel();
            self.sink.info("正在取消认证 ...");
        }
    }
}
