//! Own one temporary native session through startup and its original retirement.
use crate::{AnalysisLifetime, run_cleanup::Resource};
use driver_api::{AgentDriver, SessionHandle};
use futures::FutureExt;
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;

#[derive(Default)]
pub(crate) struct Activity {
    active: AtomicUsize,
}
impl Activity {
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire) != 0
    }
    pub fn lease(
        self: &Arc<Self>,
        downstream: Option<Arc<dyn AnalysisLifetime>>,
    ) -> Arc<dyn AnalysisLifetime> {
        self.active.fetch_add(1, Ordering::AcqRel);
        Arc::new(ActivityLease {
            activity: self.clone(),
            downstream,
        })
    }
}
struct ActivityLease {
    activity: Arc<Activity>,
    downstream: Option<Arc<dyn AnalysisLifetime>>,
}
impl AnalysisLifetime for ActivityLease {
    fn cleanup_pending(&self, message: &str) {
        notify(self.downstream.as_ref(), message);
    }
}
impl Drop for ActivityLease {
    fn drop(&mut self) {
        self.activity.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy)]
pub(super) enum Purpose {
    Analysis,
    Summary,
    Review,
}
impl Purpose {
    fn startup_pending(self) -> &'static str {
        match self {
            Self::Analysis => "分析 Agent 启动尚未确认，后台仍在等待原请求；本项目暂不启动新的分析",
            Self::Summary => "总结 Agent 启动尚未确认，后台仍在等待并清理原请求",
            Self::Review => "审查 Agent 启动尚未确认，后台仍在等待并清理原请求",
        }
    }
    fn cleanup_pending(self) -> &'static str {
        match self {
            Self::Analysis => {
                "分析 Agent 会话关闭尚未确认，后台仍在观察原请求；本项目暂不启动新的分析"
            }
            Self::Summary => "总结 Agent 会话关闭尚未确认，后台仍在观察原请求",
            Self::Review => "审查 Agent 会话关闭尚未确认，后台仍在观察原请求",
        }
    }
}
fn notify(lifetime: Option<&Arc<dyn AnalysisLifetime>>, message: &str) {
    if let Some(lifetime) = lifetime {
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| lifetime.cleanup_pending(message)));
    }
}
struct Assets {
    _driver: Arc<dyn AgentDriver>,
    directory: Directory,
    lifetime: Option<Arc<dyn AnalysisLifetime>>,
}
enum Directory {
    Temporary(tempfile::TempDir),
    Workspace(std::path::PathBuf),
}
impl Directory {
    fn path(&self) -> &std::path::Path {
        match self {
            Self::Temporary(directory) => directory.path(),
            Self::Workspace(directory) => directory,
        }
    }
}
struct StartupObserver {
    lifetime: Option<Arc<dyn AnalysisLifetime>>,
    purpose: Purpose,
    consumed: bool,
    stopped: Arc<AtomicBool>,
}
impl Drop for StartupObserver {
    fn drop(&mut self) {
        if !self.consumed {
            self.stopped.store(true, Ordering::Release);
            notify(self.lifetime.as_ref(), self.purpose.startup_pending());
        }
    }
}
pub(super) struct Start {
    stopped: Arc<AtomicBool>,
}
impl Start {
    /// Check between read-only preparation and native creation. An already
    /// admitted native request stays owned; cancellation never drops it.
    pub fn check(&self) -> Result<(), String> {
        if self.stopped.load(Ordering::Acquire) {
            Err("临时任务已取消，未创建新会话".into())
        } else {
            Ok(())
        }
    }
}
pub(super) struct Session {
    driver: Arc<dyn AgentDriver>,
    handle: SessionHandle,
    resource: Option<Resource>,
    assets: Option<Assets>,
    purpose: Purpose,
    runtime: tokio::runtime::Handle,
}
impl Session {
    pub async fn start<F, Fut>(
        driver: Arc<dyn AgentDriver>,
        lifetime: Option<Arc<dyn AnalysisLifetime>>,
        purpose: Purpose,
        create: F,
    ) -> Result<Self, String>
    where
        F: FnOnce(Arc<dyn AgentDriver>, String, Start) -> Fut + Send + 'static,
        Fut: Future<Output = Result<SessionHandle, String>> + Send,
    {
        Self::start_with_budget(driver, lifetime, purpose, create, Duration::from_secs(30)).await
    }
    pub async fn start_in_workspace<F, Fut>(
        driver: Arc<dyn AgentDriver>,
        lifetime: Option<Arc<dyn AnalysisLifetime>>,
        workspace: String,
        create: F,
    ) -> Result<Self, String>
    where
        F: FnOnce(Arc<dyn AgentDriver>, String, Start) -> Fut + Send + 'static,
        Fut: Future<Output = Result<SessionHandle, String>> + Send,
    {
        Self::start_owned(
            driver,
            lifetime,
            Purpose::Review,
            create,
            Duration::from_mins(1),
            Some(workspace.into()),
        )
        .await
    }
    async fn start_with_budget<F, Fut>(
        driver: Arc<dyn AgentDriver>,
        lifetime: Option<Arc<dyn AnalysisLifetime>>,
        purpose: Purpose,
        create: F,
        budget: Duration,
    ) -> Result<Self, String>
    where
        F: FnOnce(Arc<dyn AgentDriver>, String, Start) -> Fut + Send + 'static,
        Fut: Future<Output = Result<SessionHandle, String>> + Send,
    {
        Self::start_owned(driver, lifetime, purpose, create, budget, None).await
    }
    async fn start_owned<F, Fut>(
        driver: Arc<dyn AgentDriver>,
        lifetime: Option<Arc<dyn AnalysisLifetime>>,
        purpose: Purpose,
        create: F,
        budget: Duration,
        workspace: Option<std::path::PathBuf>,
    ) -> Result<Self, String>
    where
        F: FnOnce(Arc<dyn AgentDriver>, String, Start) -> Fut + Send + 'static,
        Fut: Future<Output = Result<SessionHandle, String>> + Send,
    {
        let runtime = tokio::runtime::Handle::current();
        let original_runtime = runtime.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let mut observer = StartupObserver {
            lifetime: lifetime.clone(),
            purpose,
            consumed: false,
            stopped: stopped.clone(),
        };
        let (created, outcome) = oneshot::channel();
        runtime.spawn(async move {
            let directory = match workspace {
                Some(path) => Directory::Workspace(path),
                None => match tempfile::tempdir() {
                    Ok(directory) => Directory::Temporary(directory),
                    Err(error) => {
                        let _ = created.send(Err(error.to_string()));
                        return;
                    }
                },
            };
            let cwd = directory.path().to_string_lossy().into_owned();
            let assets = Assets {
                _driver: driver.clone(),
                directory,
                lifetime,
            };
            // The observation budget must never cancel native new/load. Its
            // late result owns the same directory, driver and retirement lease.
            let context = Start { stopped };
            match AssertUnwindSafe(async {
                context.check()?;
                create(driver.clone(), cwd, context).await
            })
            .catch_unwind()
            .await
            {
                Ok(Ok(handle)) => {
                    let resource = Resource::capture(driver.clone(), handle.clone()).await;
                    let session = Self {
                        driver,
                        handle,
                        resource: Some(resource),
                        assets: Some(assets),
                        purpose,
                        runtime: original_runtime,
                    };
                    // A missing receiver drops Session, which schedules cleanup
                    // on the captured runtime rather than dropping its resource.
                    let _ = created.send(Ok(session));
                }
                Ok(Err(error)) => {
                    let _ = created.send(Err(error));
                }
                Err(_) => {
                    notify(assets.lifetime.as_ref(), purpose.startup_pending());
                    let _ = created.send(Err(purpose.startup_pending().into()));
                    // No returned identity means there is no safe handle to
                    // invent or retry. Preserve the original assets as unknown.
                    std::future::pending::<()>().await;
                    drop(assets);
                }
            }
        });
        match tokio::time::timeout(budget, outcome).await {
            Ok(Ok(result)) => {
                observer.consumed = true;
                result
            }
            _ => Err(purpose.startup_pending().into()),
        }
    }
    pub fn handle(&self) -> &SessionHandle {
        &self.handle
    }
    pub fn directory(&self) -> &std::path::Path {
        self.assets
            .as_ref()
            .expect("original auxiliary assets")
            .directory
            .path()
    }
    pub fn cleanup(&mut self, interrupt: bool) -> Option<oneshot::Receiver<Result<(), String>>> {
        let resource = self.resource.take()?;
        let assets = self.assets.take().expect("original auxiliary assets");
        let driver = self.driver.clone();
        let handle = self.handle.clone();
        let purpose = self.purpose;
        let (reply, outcome) = oneshot::channel();
        self.runtime.spawn(async move {
            let mut stopping = interrupt.then(|| {
                tokio::spawn(async move {
                    AssertUnwindSafe(driver.interrupt(&handle))
                        .catch_unwind()
                        .await
                })
            });
            if let Some(stop) = &mut stopping
                && tokio::time::timeout(Duration::from_secs(5), stop)
                    .await
                    .is_ok()
            {
                stopping.take();
            }
            let cleanup = resource.retire(Duration::from_secs(15));
            let mut changes = cleanup.subscribe();
            let mut reply = Some(reply);
            loop {
                let status = *changes.borrow_and_update();
                match status {
                    crate::run_cleanup::Status::Confirmed => {
                        if let Some(stop) = stopping {
                            stop.abort();
                            let _ = stop.await;
                        }
                        if let Some(reply) = reply.take() {
                            let _ = reply.send(Ok(()));
                        }
                        drop(assets);
                        return;
                    }
                    crate::run_cleanup::Status::Pending => {
                        if let Some(reply) = reply.take() {
                            notify(assets.lifetime.as_ref(), purpose.cleanup_pending());
                            let _ = reply.send(Err(purpose.cleanup_pending().into()));
                        }
                    }
                    crate::run_cleanup::Status::Working => {}
                }
                if changes.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        });
        Some(outcome)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        drop(self.cleanup(true));
    }
}

#[cfg(test)]
mod tests;
