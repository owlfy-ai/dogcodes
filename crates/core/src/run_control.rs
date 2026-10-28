//! Admission and cancellation of one original Core run. These phases describe
//! local ownership, never proof that an unknown native operation has finished.
use agent_domain::RunId;
use tokio::sync::watch;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Dispatching,
    Running,
    Settling,
    Finished,
    Abandoned,
}
#[derive(Clone, Copy)]
struct State {
    stop: bool,
    phase: Phase,
}

pub(super) struct Control {
    pub id: RunId,
    pub usage_provider: Option<std::sync::Arc<dyn driver_api::QuotaProvider>>,
    state: watch::Sender<State>,
    task: std::sync::Mutex<Option<agent_domain::TaskId>>,
    native_owned: std::sync::atomic::AtomicBool,
    session_attempted: std::sync::atomic::AtomicBool,
    startup: std::sync::Mutex<Option<std::sync::Arc<crate::session_startup::Startup>>>,
    cleanup: std::sync::Mutex<Option<std::sync::Arc<crate::run_cleanup::Cleanup>>>,
    native_resource: std::sync::Mutex<Option<crate::run_cleanup::Resource>>,
    workspace: std::sync::Mutex<Option<tokio::sync::OwnedRwLockReadGuard<()>>>,
}
pub(super) enum StopTarget {
    Deferred,
    Running,
    Finished,
    Abandoned,
}

impl Control {
    #[cfg(test)]
    pub fn new(id: RunId) -> Self {
        Self::with_usage_provider(id, None)
    }
    pub fn with_usage_provider(
        id: RunId,
        usage_provider: Option<std::sync::Arc<dyn driver_api::QuotaProvider>>,
    ) -> Self {
        Self {
            id,
            usage_provider,
            task: std::sync::Mutex::new(None),
            native_owned: std::sync::atomic::AtomicBool::new(false),
            session_attempted: std::sync::atomic::AtomicBool::new(false),
            startup: std::sync::Mutex::new(None),
            cleanup: std::sync::Mutex::new(None),
            native_resource: std::sync::Mutex::new(None),
            workspace: std::sync::Mutex::new(None),
            state: watch::channel(State {
                stop: false,
                phase: Phase::Preparing,
            })
            .0,
        }
    }
    pub fn bind_task(&self, task: agent_domain::TaskId) {
        *self.task.lock().expect("run task") = Some(task);
    }
    pub fn hold_workspace(&self, permit: tokio::sync::OwnedRwLockReadGuard<()>) {
        *self.workspace.lock().expect("run workspace") = Some(permit);
    }
    pub fn task(&self) -> Option<agent_domain::TaskId> {
        self.task.lock().expect("run task").clone()
    }
    pub fn register_startup(
        &self,
        startup: std::sync::Arc<crate::session_startup::Startup>,
    ) -> bool {
        let cleanup = self.cleanup.lock().expect("run cleanup");
        if cleanup.is_some() {
            return false;
        }
        self.state.send_if_modified(|s| {
            if s.stop || s.phase != Phase::Preparing {
                return false;
            }
            let mut slot = self.startup.lock().expect("run startup");
            if slot.is_some() {
                return false;
            }
            *slot = Some(startup);
            self.session_starting();
            true
        })
    }
    pub fn startup(&self) -> Option<std::sync::Arc<crate::session_startup::Startup>> {
        self.startup.lock().expect("run startup").clone()
    }
    pub fn session_attempted(&self) -> bool {
        self.session_attempted
            .load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn session_starting(&self) {
        self.session_attempted
            .store(true, std::sync::atomic::Ordering::Release);
        self.native_owned
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn native_resolved(&self) {
        self.native_owned
            .store(false, std::sync::atomic::Ordering::Release);
        self.native_resource
            .lock()
            .expect("original native resource")
            .take();
    }
    pub fn hold_native_resource(&self, resource: crate::run_cleanup::Resource) {
        *self
            .native_resource
            .lock()
            .expect("original native resource") = Some(resource);
    }
    pub fn take_native_resource(&self) -> Option<crate::run_cleanup::Resource> {
        self.native_resource
            .lock()
            .expect("original native resource")
            .take()
    }
    pub fn owns_native(&self) -> bool {
        self.native_owned.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn cleanup_or_start(
        &self,
        start: impl FnOnce() -> std::sync::Arc<crate::run_cleanup::Cleanup>,
    ) -> std::sync::Arc<crate::run_cleanup::Cleanup> {
        self.cleanup
            .lock()
            .expect("run cleanup")
            .get_or_insert_with(start)
            .clone()
    }
    pub fn cleanup_pending(&self) -> bool {
        self.cleanup
            .lock()
            .expect("run cleanup")
            .as_ref()
            .is_some_and(|c| !c.confirmed())
    }
    pub fn request_stop(&self) {
        self.state.send_modify(|s| s.stop = true);
    }
    pub fn stopping(&self) -> bool {
        self.state.borrow().stop
    }
    pub fn finished(&self) -> bool {
        self.state.borrow().phase == Phase::Finished
    }
    /// Ordered with stop acceptance; never hold a lock across native dispatch.
    pub fn begin_dispatch(&self) -> bool {
        self.state.send_if_modified(|s| {
            if s.stop || s.phase != Phase::Preparing {
                return false;
            }
            s.phase = Phase::Dispatching;
            true
        })
    }
    /// Includes already-started unsolicited turns, which have no prompt call.
    pub fn ready(&self) {
        self.state.send_modify(|s| s.phase = Phase::Running);
    }
    /// Native execution has ended; local records still belong to this run.
    pub fn settling(&self) {
        self.state.send_modify(|s| s.phase = Phase::Settling);
    }
    pub fn accepts_interactions(&self) -> bool {
        self.state.borrow().phase == Phase::Running
    }
    pub fn finish(&self) {
        self.workspace.lock().expect("run workspace").take();
        self.state.send_modify(|s| s.phase = Phase::Finished);
    }
    pub fn abandon(&self) {
        self.state.send_if_modified(|s| {
            if s.phase == Phase::Finished {
                return false;
            }
            s.phase = Phase::Abandoned;
            true
        });
    }
    pub async fn stop_target(&self) -> StopTarget {
        let mut state = self.state.subscribe();
        loop {
            let phase = state.borrow_and_update().phase;
            match phase {
                Phase::Preparing => return StopTarget::Deferred,
                Phase::Running => return StopTarget::Running,
                Phase::Settling | Phase::Finished => return StopTarget::Finished,
                Phase::Abandoned => return StopTarget::Abandoned,
                Phase::Dispatching => {}
            }
            // The sender is owned by self; disappearing owners explicitly mark
            // abandonment so cancellation cannot silently bind to a later run.
            if state.changed().await.is_err() {
                return StopTarget::Abandoned;
            }
        }
    }
}

pub(super) enum CancelDelivery {
    Sent,
    Deferred,
}

pub(super) fn interruption(
    task: &agent_domain::TaskId,
    turn: &agent_domain::TurnId,
    session_untouched: bool,
) -> (String, serde_json::Value, agent_protocol::EventSource) {
    (
        "turn.failed".into(),
        serde_json::json!({"taskId":task,"turnId":turn,
        "failureKind":"interrupted","message":"turn cancelled before dispatch",
        "nativeDispatched":false,"nativeSessionUntouched":session_untouched,"failedAt":chrono::Utc::now()}),
        agent_protocol::EventSource::Hub,
    )
}
