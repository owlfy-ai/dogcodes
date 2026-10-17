//! Ephemeral subscriptions owned by the service connection, not a UI window.
use super::{DirectoryRequest, directory_base, resolve};
use agent_core::Core;
use notify::Watcher;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WatchMessage {
    Invalidated,
    Heartbeat,
    Error { code: String, message: String },
}
#[derive(Clone, Default)]
struct Signal {
    revision: u64,
    error: Option<String>,
}
pub struct WatchService {
    slots: Arc<Semaphore>,
    native_slots: Arc<Semaphore>,
    worker_slots: Arc<Semaphore>,
}
impl Default for WatchService {
    fn default() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(64)),
            native_slots: Arc::new(Semaphore::new(8)),
            worker_slots: Arc::new(Semaphore::new(72)),
        }
    }
}
impl WatchService {
    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        64 - self.slots.available_permits()
    }
    fn worker_slot(&self) -> Result<OwnedSemaphorePermit, String> {
        self.worker_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "目录监听资源正在释放，请稍后重试".into())
    }
    pub async fn open(
        &self,
        core: Arc<Core>,
        request: DirectoryRequest,
    ) -> Result<Subscription, String> {
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "展开的监听目录过多，请先折叠部分目录")?;
        let base = directory_base(&core, &request).await?;
        let registered_base = base.clone();
        let path = request.path.clone();
        let target = tokio::task::spawn_blocking(move || {
            let target = resolve(&base, &path)?;
            if !target.is_dir() {
                return Err("不是目录".to_owned());
            }
            Ok(target)
        })
        .await
        .map_err(|e| e.to_string())??;
        let native = if let Ok(native_permit) = self.native_slots.clone().try_acquire_owned() {
            let (worker, ready) =
                Worker::start(target.clone(), Some(native_permit), self.worker_slot()?)?;
            ready_before(worker, ready, Duration::from_secs(2)).await
        } else {
            None
        };
        let worker = if let Some(worker) = native {
            worker
        } else {
            let (worker, ready) = Worker::start(target.clone(), None, self.worker_slot()?)?;
            tokio::time::timeout(Duration::from_secs(10), ready)
                .await
                .map_err(|_| "目录定时检查初始化超时")?
                .map_err(|_| "目录定时检查初始化已结束")??;
            worker
        };
        Ok(Subscription {
            core,
            request,
            target,
            registered_base,
            worker,
            _permit: permit,
        })
    }
}
/// The native watcher (including blocking creation/destruction) stays on a
/// dedicated thread. No system callback can block a Tokio executor or its
/// shutdown. Closed receivers cancel pending setup as well as active watches.
struct Worker {
    _stop: std::sync::mpsc::Sender<()>,
    changes: watch::Receiver<Signal>,
}
impl Worker {
    fn start(
        target: PathBuf,
        native_permit: Option<OwnedSemaphorePermit>,
        worker_permit: OwnedSemaphorePermit,
    ) -> Result<(Self, tokio::sync::oneshot::Receiver<Result<(), String>>), String> {
        let (stop, stopped) = std::sync::mpsc::channel();
        let (ready, accepted) = tokio::sync::oneshot::channel();
        let (sender, changes) = watch::channel(Signal::default());
        std::thread::Builder::new()
            .name("workspace-watch".into())
            .spawn(move || {
                let _worker_permit = worker_permit;
                // Held by this thread until the watcher ends.
                let native_permit = native_permit;
                let setup = || -> Result<Box<dyn Watcher>, String> {
                    let handler = move |event: notify::Result<notify::Event>| {
                        if event
                            .as_ref()
                            .is_ok_and(|event| matches!(event.kind, notify::EventKind::Access(_)))
                        {
                            return;
                        }
                        sender.send_modify(|signal| {
                            signal.revision = signal.revision.wrapping_add(1);
                            if let Err(error) = event {
                                signal.error = Some(format!("目录监听失败：{error}"));
                            }
                        });
                    };
                    let mut watcher: Box<dyn Watcher> = if native_permit.is_some() {
                        Box::new(notify::recommended_watcher(handler).map_err(|e| e.to_string())?)
                    } else {
                        Box::new(
                            notify::PollWatcher::new(
                                handler,
                                notify::Config::default()
                                    .with_poll_interval(Duration::from_secs(1))
                                    .with_follow_symlinks(false),
                            )
                            .map_err(|e| e.to_string())?,
                        )
                    };
                    watcher
                        .watch(&target, notify::RecursiveMode::NonRecursive)
                        .map_err(|e| e.to_string())?;
                    Ok(watcher)
                };
                match setup() {
                    Ok(watcher) => {
                        if ready.send(Ok(())).is_ok() {
                            let _ = stopped.recv();
                        }
                        drop(watcher);
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((
            Self {
                _stop: stop,
                changes,
            },
            accepted,
        ))
    }
}
async fn ready_before(
    worker: Worker,
    ready: tokio::sync::oneshot::Receiver<Result<(), String>>,
    timeout: Duration,
) -> Option<Worker> {
    if let Ok(Ok(Ok(()))) = tokio::time::timeout(timeout, ready).await {
        Some(worker)
    } else {
        // The worker keeps its native and thread quotas until the OS returns.
        // Drop cancellation now; never attach a late successful native watcher.
        drop(worker);
        None
    }
}

pub struct Subscription {
    core: Arc<Core>,
    request: DirectoryRequest,
    target: PathBuf,
    registered_base: PathBuf,
    worker: Worker,
    _permit: OwnedSemaphorePermit,
}
impl Subscription {
    pub async fn next(&mut self) -> Result<WatchMessage, String> {
        let changed =
            tokio::time::timeout(Duration::from_secs(15), self.worker.changes.changed()).await;
        let invalidated = match changed {
            Ok(Ok(())) => true,
            Ok(Err(_)) => return Err("目录监听已结束，请重新连接".into()),
            Err(_) => false,
        };
        if invalidated {
            // Bounded debounce: continuous edits must not starve refresh forever.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            loop {
                match tokio::time::timeout_at(
                    deadline.min(tokio::time::Instant::now() + Duration::from_millis(200)),
                    self.worker.changes.changed(),
                )
                .await
                {
                    Ok(Ok(())) if tokio::time::Instant::now() < deadline => {}
                    _ => break,
                }
            }
        }
        if let Some(error) = self.worker.changes.borrow().error.clone() {
            return Err(error);
        }
        let base = directory_base(&self.core, &self.request).await?;
        // Compare the declared root before resolving the old relative path: it
        // may no longer exist under the newly selected project directory.
        if base != self.registered_base {
            return Ok(WatchMessage::Error {
                code: "scope_changed".into(),
                message: "项目目录已变化，正在刷新文件树".into(),
            });
        }
        let path = self.request.path.clone();
        let target = tokio::task::spawn_blocking(move || resolve(&base, &path))
            .await
            .map_err(|e| e.to_string())??;
        if target != self.target {
            return Ok(WatchMessage::Error {
                code: "scope_changed".into(),
                message: "项目目录已变化，正在刷新文件树".into(),
            });
        }
        Ok(if invalidated {
            WatchMessage::Invalidated
        } else {
            WatchMessage::Heartbeat
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn workspace_watch_late_native_setup_is_cancelled_before_fallback() {
        let (stop, stopped) = std::sync::mpsc::channel();
        let (_sender, changes) = watch::channel(Signal::default());
        let (ready, accepted) = tokio::sync::oneshot::channel();
        let worker = Worker {
            _stop: stop,
            changes,
        };
        assert!(
            ready_before(worker, accepted, Duration::from_millis(10))
                .await
                .is_none()
        );
        assert!(matches!(
            stopped.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
        assert!(
            ready.send(Ok(())).is_err(),
            "late setup must not become the active subscription"
        );
    }
}
