//! One original shutdown call, observed through timeouts and caller loss.
//! A failure is not proof, and never authorizes another native shutdown call.
use crate::{Core, CoreError, recovery::RunOwnership, run_control::Control};
use agent_domain::{TaskId, TurnId};
use driver_api::{AgentDriver, SessionHandle, SessionShutdownProof};
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Working,
    Pending,
    Confirmed,
}

#[cfg(test)]
mod tests;
pub(super) struct Cleanup {
    status: watch::Sender<Status>,
    owner: std::sync::Mutex<Option<NativeOwner>>,
}
struct NativeOwner {
    _driver: Option<Arc<dyn AgentDriver>>,
    _handle: Option<SessionHandle>,
    _startup: Option<Arc<crate::session_startup::Startup>>,
}
enum Proof {
    Discover,
    Captured(Option<SessionShutdownProof>),
}
/// Capture before a load/configure call can retire or rekey the old resource.
/// Abandoning this read-only proof never initiates native shutdown.
pub(super) struct Resource {
    driver: Arc<dyn AgentDriver>,
    handle: SessionHandle,
    proof: Option<SessionShutdownProof>,
}
impl Resource {
    pub async fn capture(driver: Arc<dyn AgentDriver>, handle: SessionHandle) -> Self {
        let proof = AssertUnwindSafe(driver.shutdown_proof(&handle))
            .catch_unwind()
            .await
            .ok()
            .flatten();
        Self {
            driver,
            handle,
            proof,
        }
    }
    pub fn retire(self, budget: Duration) -> Arc<Cleanup> {
        Cleanup::start_with_proof(
            Some(self.driver),
            Some(self.handle),
            None,
            true,
            budget,
            Proof::Captured(self.proof),
        )
    }
}
impl Cleanup {
    pub fn start(
        driver: Option<Arc<dyn AgentDriver>>,
        handle: Option<SessionHandle>,
        started: bool,
        budget: Duration,
    ) -> Arc<Self> {
        Self::start_original(driver, handle, None, started, budget)
    }
    pub fn start_original(
        driver: Option<Arc<dyn AgentDriver>>,
        handle: Option<SessionHandle>,
        startup: Option<Arc<crate::session_startup::Startup>>,
        started: bool,
        budget: Duration,
    ) -> Arc<Self> {
        Self::start_with_proof(driver, handle, startup, started, budget, Proof::Discover)
    }
    fn start_with_proof(
        driver: Option<Arc<dyn AgentDriver>>,
        handle: Option<SessionHandle>,
        startup: Option<Arc<crate::session_startup::Startup>>,
        started: bool,
        budget: Duration,
        captured: Proof,
    ) -> Arc<Self> {
        let state = Arc::new(Self {
            owner: std::sync::Mutex::new(started.then(|| NativeOwner {
                _driver: driver.clone(),
                _handle: handle.clone(),
                _startup: startup.clone(),
            })),
            status: watch::channel(if started {
                Status::Working
            } else {
                Status::Confirmed
            })
            .0,
        });
        if !started {
            return state;
        }
        let owned = state.clone();
        tokio::spawn(async move {
            let operation = async {
                let (driver, handle) = match (driver, handle, startup.as_ref()) {
                    (Some(driver), Some(handle), _) => (driver, handle),
                    (_, None, Some(startup)) => match startup.resolution().await {
                        crate::session_startup::Outcome::Ready(handle) => {
                            (startup.driver.clone(), handle)
                        }
                        crate::session_startup::Outcome::Rejected => return true,
                        crate::session_startup::Outcome::Closing(cleanup) => {
                            cleanup.wait_confirmed().await;
                            return true;
                        }
                        _ => {
                            owned.status.send_replace(Status::Pending);
                            return std::future::pending::<bool>().await;
                        }
                    },
                    _ => {
                        owned.status.send_replace(Status::Pending);
                        return std::future::pending::<bool>().await;
                    }
                };
                let proof = match captured {
                    Proof::Discover => AssertUnwindSafe(driver.shutdown_proof(&handle))
                        .catch_unwind()
                        .await
                        .ok()
                        .flatten(),
                    Proof::Captured(proof) => proof,
                };
                // Keep polling the same close even if its observer can already
                // prove the original resources ended. Dropping this JoinHandle
                // detaches the request; it never cancels/replays native shutdown.
                let mut closing = tokio::spawn(async move {
                    matches!(
                        AssertUnwindSafe(driver.shutdown(handle))
                            .catch_unwind()
                            .await,
                        Ok(Ok(()))
                    )
                });
                if let Some(proof) = proof {
                    let proof = AssertUnwindSafe(proof).catch_unwind();
                    tokio::pin!(proof);
                    return tokio::select! {
                        positive=&mut proof=>{
                            if positive.is_err(){owned.status.send_replace(Status::Pending);false}else{true}
                        },
                        outcome=&mut closing=>{
                            if !matches!(outcome,Ok(true)){owned.status.send_replace(Status::Pending);}
                            if proof.await.is_err(){owned.status.send_replace(Status::Pending);false}else{true}
                        },
                    };
                }
                if !matches!(closing.await, Ok(true)) {
                    owned.status.send_replace(Status::Pending);
                    return false;
                }
                true
            };
            tokio::pin!(operation);
            let confirmed = tokio::select! {
                result = &mut operation => result,
                () = tokio::time::sleep(budget) => {
                    owned.status.send_replace(Status::Pending);
                    // The timer is observational; the same operation keeps its
                    // original call and proof. It never cancels or replays them.
                    operation.await
                }
            };
            if confirmed {
                if let Some(startup) = &startup {
                    startup.release_history();
                }
                owned.owner.lock().expect("original native cleanup").take();
                owned.status.send_replace(Status::Confirmed);
            }
        });
        state
    }
    pub async fn wait_confirmed(&self) {
        let mut status = self.subscribe();
        loop {
            if *status.borrow_and_update() == Status::Confirmed {
                return;
            }
            if status.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
    pub fn confirmed(&self) -> bool {
        *self.status.borrow() == Status::Confirmed
    }
    pub fn subscribe(&self) -> watch::Receiver<Status> {
        self.status.subscribe()
    }
}

/// Transfers the original `RunOwnership` into cleanup if execution unwinds or its
/// future is dropped. The public worker handle never owns the shutdown future.
pub(super) struct Execution {
    core: Arc<Core>,
    task: TaskId,
    turn: TurnId,
    ownership: Option<RunOwnership>,
    runtime: Option<tokio::runtime::Handle>,
}
impl Execution {
    pub fn new(core: Arc<Core>, task: TaskId, turn: TurnId, ownership: RunOwnership) -> Self {
        ownership.control.bind_task(task.clone());
        Self {
            core,
            task,
            turn,
            ownership: Some(ownership),
            runtime: tokio::runtime::Handle::try_current().ok(),
        }
    }
    pub fn control(&self) -> Arc<Control> {
        self.ownership
            .as_ref()
            .expect("owned execution")
            .control
            .clone()
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        let Some(ownership) = self.ownership.take() else {
            return;
        };
        if ownership.control.finished() {
            return;
        }
        ownership.control.abandon();
        let Some(runtime) = self
            .runtime
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
        else {
            return;
        };
        let core = self.core.clone();
        let task = self.task.clone();
        let turn = self.turn.clone();
        runtime.spawn(async move {
            let control = ownership.control.clone();
            let _ownership = ownership;
            core.wait_failed_run_cleanup(&control).await;
            // Only local bookkeeping is retried. Native cleanup above is the
            // same confirmed operation and must never be replayed on storage errors.
            while core.recover_abandoned_workspace(&control).await.is_err() {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let payload = serde_json::json!({
                "taskId":task,"turnId":turn,"failureKind":"execution_abandoned",
                "message":"execution worker ended unexpectedly","failedAt":chrono::Utc::now(),
                "nativeSessionUntouched":!control.session_attempted(),
            });
            core.persist_original_run_event(
                &control,
                "turn.failed",
                payload,
                agent_protocol::EventSource::Hub,
            )
            .await;
            control.finish();
        });
    }
}

impl Core {
    fn original_cleanup(&self, control: &Control) -> Arc<Cleanup> {
        let driver = self
            .owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&control.id)
            .cloned();
        let handle = self
            .owned_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&control.id)
            .cloned();
        control.cleanup_or_start(|| {
            if control.owns_native()
                && let Some(resource) = control.take_native_resource()
            {
                return resource.retire(Duration::from_secs(10));
            }
            Cleanup::start_original(
                driver,
                handle,
                control.startup(),
                control.owns_native(),
                Duration::from_secs(10),
            )
        })
    }
    pub(super) async fn wait_run_cleanup(&self, control: &Control) {
        let cleanup = self.original_cleanup(control);
        let mut changes = cleanup.subscribe();
        let mut reported = false;
        loop {
            let status = *changes.borrow_and_update();
            if status == Status::Confirmed {
                return;
            }
            if status == Status::Pending && !reported {
                reported = true;
                self.report_run_cleanup_pending(control).await;
            }
            // The original control retains the sender even if a shutdown call
            // failed and this adapter has no passive proof. Unknown stays owned.
            if changes.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
    pub(super) async fn wait_failed_run_cleanup(&self, control: &Control) {
        let cleanup = self.original_cleanup(control);
        if !cleanup.confirmed() {
            self.report_run_cleanup_pending(control).await;
        }
        self.wait_run_cleanup(control).await;
        self.mark_retired_run_session(control).await;
    }
    async fn mark_retired_run_session(&self, control: &Control) {
        if !control.owns_native() {
            return;
        }
        let Some(task) = control.task() else {
            return;
        };
        let driver = self
            .owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&control.id)
            .cloned();
        let handle = self
            .owned_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&control.id)
            .cloned();
        let (Some(driver), Some(handle)) = (driver, handle) else {
            return;
        };
        let Ok(Some(state)) = self.repository.task_execution_state(&task).await else {
            return;
        };
        let Some(mapping) = state.session_maps.iter().find(|m| {
            m.executor == *driver.executor()
                && m.native_session_id == handle.native_session_id
                && m.status == agent_protocol::SessionMapStatus::Active
        }) else {
            return;
        };
        let (kind, status, timestamp) =
            if driver.resume_capability() == driver_api::SessionResumeCapability::Supported {
                ("session.resumable", "resumable", "resumableAt")
            } else {
                ("session.detached", "detached", "detachedAt")
            };
        let _ = self.append_session_identity(&task, &control.id, driver.executor(), Some(mapping), kind, serde_json::json!({
            "sessionMapId":mapping.session_map_id,"taskId":task,"nativeSessionId":handle.native_session_id,
                "status":status,(timestamp):chrono::Utc::now(),
        })).await;
    }
    async fn recover_abandoned_workspace(&self, control: &Control) -> Result<(), CoreError> {
        if !control.owns_native() {
            return Ok(());
        }
        let Some(task) = control.task() else {
            return Ok(());
        };
        let Some(state) = self.repository.task_execution_state(&task).await? else {
            return Ok(());
        };
        let Some(run) = state
            .task
            .active_primary_run
            .filter(|r| r.run_id == control.id)
        else {
            return Ok(());
        };
        let scope = crate::TurnWorkspaceScope {
            task_id: task.clone(),
            turn_id: run.turn_id.clone(),
            run_id: run.run_id,
            executor: run.executor,
            project_paths: std::iter::once(state.workspace.path)
                .chain(state.workspace.additional_directories)
                .collect(),
        };
        let through = self.repository.event_head().await?;
        self.recover_run_workspace(&task, &run.turn_id, Some(&scope), through)
            .await
    }
    async fn report_run_cleanup_pending(&self, control: &Control) {
        control.request_stop();
        let _ = self
            .append_original_run_event(
                control,
                "turn.cancel_requested",
                serde_json::json!({
                    "reason":"cleanup_pending","requestedAt":chrono::Utc::now(),
                }),
                agent_protocol::EventSource::Hub,
            )
            .await;
    }
    pub(super) async fn append_original_run_event(
        &self,
        control: &Control,
        kind: &str,
        payload: serde_json::Value,
        source: agent_protocol::EventSource,
    ) -> Result<(), CoreError> {
        let Some(task_id) = control.task() else {
            return Ok(());
        };
        // Execution time is independent of how long its local record waits.
        let occurred_at = match kind {
            "turn.completed" => payload.get("completedAt"),
            "turn.failed" => payload.get("failedAt"),
            _ => None,
        }
        .and_then(|value| {
            serde_json::from_value::<chrono::DateTime<chrono::Utc>>(value.clone()).ok()
        });
        let command = agent_domain::CommandId::new();
        loop {
            let Some(state) = self.repository.task_execution_state(&task_id).await? else {
                return Ok(());
            };
            let Some(run) = state
                .task
                .active_primary_run
                .as_ref()
                .filter(|r| r.run_id == control.id)
            else {
                return Ok(());
            };
            if kind == "turn.cancel_requested" && run.state == agent_domain::RunState::Interrupting
            {
                return Ok(());
            }
            let mut body = payload.clone();
            body["taskId"] = serde_json::json!(task_id);
            body["turnId"] = serde_json::json!(run.turn_id);
            body["runId"] = serde_json::json!(control.id);
            let mut event = agent_protocol::NewEvent::hub(
                agent_protocol::StreamRef {
                    stream_type: agent_protocol::StreamType::Task,
                    stream_id: task_id.to_string(),
                },
                kind,
                body,
            );
            event.source = source.clone();
            if let Some(occurred_at) = occurred_at {
                event.occurred_at = occurred_at;
            }
            match self
                .repository
                .append(crate::AppendRequest {
                    request_fingerprint: None,
                    command_id: command.clone(),
                    idempotency_key: format!("run-{kind}-{command}"),
                    expected_stream_version: Some(state.task.stream_version),
                    artifacts: vec![],
                    events: vec![event],
                })
                .await
            {
                Ok(outcome) => {
                    self.publish(&outcome.events);
                    return Ok(());
                }
                Err(crate::RepositoryError::Conflict { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    /// Retry only the frozen local record, guarded by the original run identity.
    pub(super) async fn persist_original_run_event(
        &self,
        control: &Control,
        kind: &str,
        payload: serde_json::Value,
        source: agent_protocol::EventSource,
    ) {
        while self
            .append_original_run_event(control, kind, payload.clone(), source.clone())
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    pub(crate) fn require_task_cleanup_clear(&self, task: &TaskId) -> Result<(), CoreError> {
        if self
            .owned_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .any(|control| control.task().as_ref() == Some(task) && control.cleanup_pending())
        {
            return Err(CoreError::CapabilityUnavailable(
                "上次任务的原生资源仍在确认结束，请稍后重试".into(),
            ));
        }
        Ok(())
    }
}
