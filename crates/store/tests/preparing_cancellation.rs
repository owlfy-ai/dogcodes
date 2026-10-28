#[path = "support/installations.rs"]
mod installations;

use agent_core::Core;
use agent_domain::{ExecutorRef, ProjectId, RunState, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use async_trait::async_trait;
use driver_api::*;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};

#[derive(Clone, Copy)]
enum PromptMode {
    Hold,
    Complete,
    Reject,
}
struct Driver {
    executor: ExecutorRef,
    create: Semaphore,
    dispatch: Semaphore,
    close: Semaphore,
    mode: PromptMode,
    creates: AtomicUsize,
    create_rejected: AtomicBool,
    resumes: AtomicUsize,
    resume_error: AtomicBool,
    reconfigure_started: AtomicUsize,
    reconfigure_removes_source: AtomicBool,
    reconfigure_blocked: AtomicBool,
    reconfigure_gate: Semaphore,
    prompts: AtomicUsize,
    ready: AtomicUsize,
    interrupts: AtomicUsize,
    closes: AtomicUsize,
    close_error: AtomicBool,
    proof: Arc<Semaphore>,
    proof_observed: Arc<AtomicBool>,
    handles: Arc<Mutex<HashMap<String, SessionHandle>>>,
    outputs: Mutex<HashMap<String, mpsc::UnboundedSender<Result<DriverEvent, DriverError>>>>,
}
impl Driver {
    fn check(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        if self
            .handles
            .lock()
            .unwrap()
            .get(&handle.native_session_id)
            .is_some_and(|original| original.same_scope(handle))
        {
            Ok(())
        } else {
            Err(DriverError::SessionIncompatible(
                "not the original fixture session".into(),
            ))
        }
    }
}
#[async_trait]
impl AgentDriver for Driver {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "preparation-fixture"
    }
    fn supports_live_session_reuse(&self) -> bool {
        true
    }
    async fn model_catalog(&self) -> Result<ModelCatalog, DriverError> {
        Ok(ModelCatalog {
            models: vec![ModelOption::new("changed", "Changed")],
            ..Default::default()
        })
    }
    async fn current_session_handle(&self, id: &str) -> Option<SessionHandle> {
        self.handles.lock().unwrap().get(id).cloned()
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.create_session(request).await
    }
    async fn reconfigure_session(
        &self,
        id: &str,
        _: SessionRequest,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.reconfigure_started.fetch_add(1, Ordering::AcqRel);
        if self.reconfigure_removes_source.load(Ordering::Acquire) {
            self.handles.lock().unwrap().remove(id);
        }
        if self.reconfigure_blocked.load(Ordering::Acquire) {
            self.reconfigure_gate.acquire().await.unwrap().forget();
        }
        Err(DriverError::CapabilityUnavailable(
            "private native configuration requires replacement".into(),
        ))
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        let index = self.creates.fetch_add(1, Ordering::AcqRel) + 1;
        // Pause before allocating a resource so rejection has no native owner.
        self.create.acquire().await.unwrap().forget();
        if self.create_rejected.load(Ordering::Acquire) {
            return Err(DriverError::Protocol("selection_rejected".into()));
        }
        let handle = SessionHandle::scoped(format!("session-{index}"));
        self.handles
            .lock()
            .unwrap()
            .insert(handle.native_session_id.clone(), handle.clone());
        Ok(handle)
    }
    async fn resume_selected_session(
        &self,
        id: &str,
        _: SessionRequest,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.resumes.fetch_add(1, Ordering::AcqRel);
        if self.resume_error.load(Ordering::Acquire) {
            return Err(DriverError::NotConfigured("private resume rejected".into()));
        }
        let mut handles = self.handles.lock().unwrap();
        if !handles.contains_key(id) {
            return Err(DriverError::SessionMissing(id.into()));
        }
        let handle = SessionHandle::scoped(id.into());
        handles.insert(id.into(), handle.clone());
        Ok(handle)
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        _: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        self.check(handle)?;
        self.prompts.fetch_add(1, Ordering::AcqRel);
        self.dispatch.acquire().await.unwrap().forget();
        self.ready.fetch_add(1, Ordering::AcqRel);
        if matches!(self.mode, PromptMode::Reject) {
            return Err(DriverError::Protocol("private dispatch rejected".into()));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        if matches!(self.mode, PromptMode::Complete) {
            tx.send(Ok(DriverEvent::TurnCompleted {
                summary: "completed".into(),
            }))
            .unwrap();
        } else {
            self.outputs
                .lock()
                .unwrap()
                .insert(handle.native_session_id.clone(), tx);
        }
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })))
    }
    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        self.check(handle)?;
        self.interrupts.fetch_add(1, Ordering::AcqRel);
        self.outputs
            .lock()
            .unwrap()
            .remove(&handle.native_session_id)
            .ok_or_else(|| {
                DriverError::Protocol("cancel arrived before original dispatch was ready".into())
            })?
            .send(Ok(DriverEvent::TurnInterrupted {
                summary: "interrupted".into(),
            }))
            .map_err(|_| DriverError::Disconnected("fixture receiver ended".into()))
    }
    async fn shutdown_proof(&self, handle: &SessionHandle) -> Option<SessionShutdownProof> {
        if !self.close_error.load(Ordering::Acquire) {
            return None;
        }
        self.check(handle).unwrap();
        let handle = handle.clone();
        let proof = self.proof.clone();
        let observed = self.proof_observed.clone();
        let handles = self.handles.clone();
        Some(Box::pin(async move {
            observed.store(true, Ordering::Release);
            proof.acquire().await.unwrap().forget();
            let mut handles = handles.lock().unwrap();
            if handles
                .get(&handle.native_session_id)
                .is_some_and(|current| current.same_scope(&handle))
            {
                handles.remove(&handle.native_session_id);
            }
        }))
    }
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        self.check(&handle)?;
        self.closes.fetch_add(1, Ordering::AcqRel);
        self.close.acquire().await.unwrap().forget();
        if self.close_error.load(Ordering::Acquire) {
            return Err(DriverError::Disconnected(
                "original cleanup still unconfirmed".into(),
            ));
        }
        self.handles
            .lock()
            .unwrap()
            .remove(&handle.native_session_id);
        self.outputs
            .lock()
            .unwrap()
            .remove(&handle.native_session_id);
        Ok(())
    }
}

struct Fixture {
    core: Arc<Core>,
    driver: Arc<Driver>,
    task: TaskId,
    project: ProjectId,
    root: tempfile::TempDir,
}
struct Workspace;
#[async_trait]
impl agent_core::WorkspaceContextProvider for Workspace {
    async fn collect(&self, _: &str) -> agent_core::WorkspaceContextSnapshot {
        agent_core::WorkspaceContextSnapshot::default()
    }
    async fn validate_workspace_roots(
        &self,
        directories: &[String],
    ) -> Result<(), WorkspaceCollectionDiagnostic> {
        if directories
            .iter()
            .all(|path| std::path::Path::new(path).is_dir())
        {
            Ok(())
        } else {
            Err(WorkspaceCollectionDiagnostic {
                kind: WorkspaceDiagnosticKind::WorkspaceUnavailable,
            })
        }
    }
}
async fn fixture(mode: PromptMode, creating: usize, dispatching: usize, closing: usize) -> Fixture {
    fixture_with_store(
        mode,
        creating,
        dispatching,
        closing,
        SqliteStore::open_in_memory().unwrap(),
    )
    .await
}
async fn fixture_with_store(
    mode: PromptMode,
    creating: usize,
    dispatching: usize,
    closing: usize,
    store: SqliteStore,
) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let core = Arc::new(Core::with_workspace_context(
        Arc::new(store),
        Arc::new(Workspace),
    ));
    let driver = Arc::new(Driver {
        executor: installations::probe(&core, "fake").await,
        create: Semaphore::new(creating),
        dispatch: Semaphore::new(dispatching),
        close: Semaphore::new(closing),
        mode,
        creates: AtomicUsize::new(0),
        create_rejected: AtomicBool::new(false),
        resumes: AtomicUsize::new(0),
        resume_error: AtomicBool::new(false),
        reconfigure_started: AtomicUsize::new(0),
        reconfigure_removes_source: AtomicBool::new(false),
        reconfigure_blocked: AtomicBool::new(false),
        reconfigure_gate: Semaphore::new(0),
        prompts: AtomicUsize::new(0),
        ready: AtomicUsize::new(0),
        interrupts: AtomicUsize::new(0),
        closes: AtomicUsize::new(0),
        close_error: AtomicBool::new(false),
        proof: Arc::new(Semaphore::new(0)),
        proof_observed: Arc::new(AtomicBool::new(false)),
        handles: Arc::default(),
        outputs: Mutex::default(),
    });
    core.register_driver(driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "private cancellation fixture".into(),
            path: root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    create_task(&core, &project, &task, &driver.executor).await;
    Fixture {
        core,
        driver,
        task,
        project,
        root,
    }
}
async fn create_task(core: &Arc<Core>, project: &ProjectId, task: &TaskId, executor: &ExecutorRef) {
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project.clone(),
            title: "private task".into(),
            preferred_executor: executor.clone(),
        },
    )))
    .await
    .unwrap();
}
async fn submit(core: &Arc<Core>, task: &TaskId) {
    submit_with_attachments(core, task, vec![]).await;
}
async fn submit_with_attachments(
    core: &Arc<Core>,
    task: &TaskId,
    attachments: Vec<agent_domain::PromptAttachment>,
) {
    let state = core.bootstrap().await.unwrap();
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: "private synthetic input".into(),
        attachments,
        model: None,
        thinking: None,
        permission_mode: None,
        executor_override: None,
        review_fix: None,
    }));
    command.expected_stream_version = Some(
        state
            .tasks
            .iter()
            .find(|t| &t.task_id == task)
            .unwrap()
            .stream_version,
    );
    core.execute(command).await.unwrap();
}
async fn cancel(core: &Arc<Core>, task: &TaskId) -> CommandEnvelope<CoreCommand> {
    let state = core.bootstrap().await.unwrap();
    let task = state.tasks.iter().find(|t| &t.task_id == task).unwrap();
    let run = task.active_primary_run.as_ref().unwrap();
    let mut command = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: task.task_id.clone(),
        turn_id: run.turn_id.clone(),
        run_id: run.run_id.clone(),
    }));
    command.expected_stream_version = Some(task.stream_version);
    command
}
async fn wait_for(test: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("private fixture did not reach expected boundary");
}
async fn idle(core: &Core, task: &TaskId) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = core.bootstrap().await.unwrap();
            if state
                .tasks
                .iter()
                .find(|t| &t.task_id == task)
                .unwrap()
                .active_primary_run
                .is_none()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("original run never reached a terminal state");
}
async fn assert_prevented(f: &Fixture) {
    let events = f.core.sync_events(0, 1000).await.unwrap();
    let events: Vec<_> = events
        .iter()
        .filter(|e| e.payload["taskId"].as_str() == Some(f.task.as_str()))
        .collect();
    assert!(events.iter().any(|e| e.event_type == "turn.failed"
        && e.payload["failureKind"] == "interrupted"
        && e.payload["nativeDispatched"] == false));
    assert!(!events.iter().any(|e| {
        [
            "history.native_prompt_accepted",
            "turn.interrupt_sent",
            "turn.cancel_failed",
        ]
        .contains(&e.event_type.as_str())
    }));
    assert_eq!(f.driver.interrupts.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn cancellation_during_session_creation_prevents_prompt_and_retires_the_late_original_session()
 {
    let f = fixture(PromptMode::Hold, 0, 10, 10).await;
    submit(&f.core, &f.task).await;
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 1).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    assert_eq!(f.driver.interrupts.load(Ordering::Acquire), 0);
    f.driver.create.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert!(f.driver.handles.lock().unwrap().is_empty());
    assert_prevented(&f).await;
}

#[tokio::test]
async fn cancellation_during_dispatch_waits_for_original_readiness_and_is_delivered_once() {
    let f = fixture(PromptMode::Hold, 10, 0, 10).await;
    submit(&f.core, &f.task).await;
    wait_for(|| f.driver.prompts.load(Ordering::Acquire) == 1).await;
    let command = cancel(&f.core, &f.task).await;
    f.core.execute(command.clone()).await.unwrap();
    f.core.execute(command).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(f.driver.interrupts.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.ready.load(Ordering::Acquire), 0);
    f.driver.dispatch.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.interrupts.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(!events.iter().any(|e| e.event_type == "turn.cancel_failed"));
}

#[tokio::test]
async fn rejected_cancel_does_not_poison_preparation_or_suppress_the_prompt() {
    let f = fixture(PromptMode::Complete, 0, 10, 10).await;
    submit(&f.core, &f.task).await;
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 1).await;
    let mut command = cancel(&f.core, &f.task).await;
    command.expected_stream_version = Some(0);
    assert!(f.core.execute(command).await.is_err());
    f.driver.create.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.interrupts.load(Ordering::Acquire), 0);
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(events.iter().any(|e| e.event_type == "turn.completed"));
    assert!(
        !events
            .iter()
            .any(|e| e.event_type == "turn.cancel_requested")
    );
}

#[tokio::test]
async fn preparation_cancel_waits_for_owned_cleanup_without_blocking_other_tasks() {
    let f = fixture(PromptMode::Complete, 0, 10, 0).await;
    submit(&f.core, &f.task).await;
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 1).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    f.driver.create.add_permits(1);
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    let state = f.core.bootstrap().await.unwrap();
    assert_eq!(
        state
            .tasks
            .iter()
            .find(|t| t.task_id == f.task)
            .unwrap()
            .active_primary_run
            .as_ref()
            .unwrap()
            .state,
        RunState::Interrupting
    );
    let peer = TaskId::new();
    create_task(&f.core, &f.project, &peer, &f.driver.executor).await;
    f.driver.create.add_permits(1);
    submit(&f.core, &peer).await;
    idle(&f.core, &peer).await;
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert!(
        f.core
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .iter()
            .find(|t| t.task_id == f.task)
            .unwrap()
            .active_primary_run
            .is_some()
    );
    f.driver.close.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_prevented(&f).await;
}

#[tokio::test]
async fn a_failed_dispatch_releases_the_waiting_cancel_without_false_native_delivery() {
    let f = fixture(PromptMode::Reject, 10, 0, 10).await;
    submit(&f.core, &f.task).await;
    wait_for(|| f.driver.prompts.load(Ordering::Acquire) == 1).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    f.driver.dispatch.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.interrupts.load(Ordering::Acquire), 0);
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(events.iter().any(|e| e.event_type == "turn.failed"));
    assert!(!events.iter().any(|e| e.event_type == "turn.interrupt_sent"));
}

#[tokio::test]
async fn a_cancel_accepted_before_the_worker_starts_never_creates_a_session() {
    let f = fixture(PromptMode::Hold, 10, 10, 10).await;
    let mutation = f.core.try_workspace_mutation().unwrap();
    submit(&f.core, &f.task).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 0);
    drop(mutation);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
    assert_prevented(&f).await;
}

#[tokio::test]
async fn failed_original_cleanup_stays_owned_until_passive_proof_without_reissuing_shutdown() {
    let f = fixture(PromptMode::Hold, 0, 10, 10).await;
    f.driver.close_error.store(true, Ordering::Release);
    submit(&f.core, &f.task).await;
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 1).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    f.driver.create.add_permits(1);
    wait_for(|| f.driver.proof_observed.load(Ordering::Acquire)).await;
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(
        f.driver.handles.lock().unwrap().len(),
        1,
        "unknown cleanup lost its original owner"
    );
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(!events.iter().any(|e| e.event_type == "turn.failed"));
    assert!(!events.iter().any(
        |e| e.payload["failureKind"] == "interrupted" || e.event_type == "turn.interrupt_sent"
    ));
    f.driver.proof.add_permits(1);
    idle(&f.core, &f.task).await;
    assert!(f.driver.handles.lock().unwrap().is_empty());
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_prevented(&f).await;
}

#[tokio::test]
async fn accepted_cancellation_takes_precedence_over_late_attachment_revalidation_failure() {
    use sha2::{Digest, Sha256};
    let f = fixture(PromptMode::Hold, 0, 10, 10).await;
    let path = f.root.path().join("attachment.txt");
    std::fs::write(&path, b"original").unwrap();
    submit_with_attachments(
        &f.core,
        &f.task,
        vec![agent_domain::PromptAttachment {
            name: "attachment.txt".into(),
            path: path.to_string_lossy().into_owned(),
            mime_type: "text/plain".into(),
            integrity: Some(agent_domain::AttachmentIntegrity {
                sha256: format!("{:x}", Sha256::digest(b"original")),
                byte_length: 8,
            }),
        }],
    )
    .await;
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 1).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    std::fs::write(path, b"changed after stop").unwrap();
    f.driver.create.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_prevented(&f).await;
}

async fn selected_submit(f: &Fixture) {
    let state = f.core.bootstrap().await.unwrap();
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: f.task.clone(),
        prompt: "must not dispatch after cancelled replacement".into(),
        model: Some("changed".into()),
        thinking: None,
        permission_mode: None,
        attachments: vec![],
        executor_override: None,
        review_fix: None,
    }));
    command.expected_stream_version = Some(
        state
            .tasks
            .iter()
            .find(|t| t.task_id == f.task)
            .unwrap()
            .stream_version,
    );
    f.core.execute(command).await.unwrap();
}

#[tokio::test]
async fn cancelling_reconfiguration_joins_original_retirement_and_never_starts_replacement() {
    let f = fixture(PromptMode::Complete, 10, 10, 0).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    selected_submit(&f).await;
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    f.driver.close.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.interrupts.load(Ordering::Acquire), 0);
    let state = f.core.bootstrap().await.unwrap();
    assert_eq!(state.session_maps[0].status, SessionMapStatus::Detached);
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(events.iter().any(|e| e.event_type == "turn.failed"
        && e.payload["nativeDispatched"] == false
        && e.payload["failureKind"] == "interrupted"));
}

#[tokio::test]
async fn replacement_waits_for_the_same_late_close_proof_and_cancel_does_not_replay_it() {
    let f = fixture(PromptMode::Complete, 10, 10, 0).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    f.driver.close_error.store(true, Ordering::Release);
    selected_submit(&f).await;
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    f.driver.close.add_permits(1);
    wait_for(|| f.driver.proof_observed.load(Ordering::Acquire)).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert!(f.core.try_workspace_mutation().is_err());
    f.driver.proof.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps[0].status,
        SessionMapStatus::Detached
    );
}

#[tokio::test]
async fn cancelled_replacement_can_finish_on_original_exit_before_shutdown_rpc_returns() {
    let f = fixture(PromptMode::Complete, 10, 10, 0).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    f.driver.close_error.store(true, Ordering::Release);
    selected_submit(&f).await;
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    wait_for(|| f.driver.proof_observed.load(Ordering::Acquire)).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    // The original close future is still waiting; only the original proof resolves.
    f.driver.proof.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.close.available_permits(), 0);
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps[0].status,
        SessionMapStatus::Detached
    );
    f.driver.close.add_permits(1);
    wait_for(|| f.driver.close.available_permits() == 0).await;
}

#[tokio::test]
async fn cancellation_before_unsupported_reconfigure_returns_retires_only_the_original_source() {
    let f = fixture(PromptMode::Complete, 10, 10, 10).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    f.driver.reconfigure_blocked.store(true, Ordering::Release);
    selected_submit(&f).await;
    wait_for(|| f.driver.reconfigure_started.load(Ordering::Acquire) == 1).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    f.driver.reconfigure_gate.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert!(f.driver.handles.lock().unwrap().is_empty());
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps[0].status,
        SessionMapStatus::Detached
    );
}

#[tokio::test]
async fn cancellation_before_native_admission_preserves_the_idle_session_and_its_checkpoint() {
    let f = fixture(PromptMode::Complete, 10, 10, 10).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    let original = f.core.bootstrap().await.unwrap().session_maps[0].clone();
    let handle = f
        .driver
        .current_session_handle(&original.native_session_id)
        .await
        .unwrap();
    let mutation = f.core.try_workspace_mutation().unwrap();
    submit(&f.core, &f.task).await;
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    drop(mutation);
    idle(&f.core, &f.task).await;
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps,
        vec![original.clone()]
    );
    assert!(
        f.driver
            .current_session_handle(&original.native_session_id)
            .await
            .unwrap()
            .same_scope(&handle)
    );
    assert_eq!(f.driver.resumes.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(events.iter().any(|e| e.event_type == "turn.failed"
        && e.payload["nativeSessionUntouched"] == true
        && e.payload["nativeDispatched"] == false));
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.resumes.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 2);
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps[0].native_session_id,
        original.native_session_id
    );
}

#[tokio::test]
async fn preparation_failure_preserves_the_untouched_idle_session_and_its_checkpoint() {
    let f = fixture(PromptMode::Complete, 10, 10, 10).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    let original = f.core.bootstrap().await.unwrap().session_maps[0].clone();
    let mutation = f.core.try_workspace_mutation().unwrap();
    submit(&f.core, &f.task).await;
    std::fs::remove_dir(f.root.path()).unwrap();
    drop(mutation);
    idle(&f.core, &f.task).await;
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps,
        vec![original]
    );
    assert_eq!(f.driver.resumes.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "turn.failed" && e.payload["nativeSessionUntouched"] == true)
    );
    std::fs::create_dir(f.root.path()).unwrap();
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.resumes.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn rejected_replacement_cannot_keep_a_closed_source_active_and_can_be_retried() {
    let f = fixture(PromptMode::Complete, 1, 10, 10).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    let original = f.core.bootstrap().await.unwrap().session_maps[0].clone();
    f.driver.create_rejected.store(true, Ordering::Release);
    selected_submit(&f).await;
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 2).await;
    // Retirement must already be durable while replacement startup is waiting.
    let state = f.core.bootstrap().await.unwrap();
    assert_eq!(
        state.session_maps[0].session_map_id,
        original.session_map_id
    );
    assert_eq!(state.session_maps[0].status, SessionMapStatus::Detached);
    assert!(f.driver.handles.lock().unwrap().is_empty());
    f.driver.create.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps[0].status,
        SessionMapStatus::Detached
    );
    let events = f.core.sync_events(0, 1000).await.unwrap();
    let failed = events
        .iter()
        .rev()
        .find(|e| e.event_type == "turn.failed")
        .unwrap();
    assert_eq!(failed.payload["failureKind"], "selection_rejected");
    assert_ne!(failed.payload["nativeSessionUntouched"], true);
    f.driver.create_rejected.store(false, Ordering::Release);
    f.driver.create.add_permits(1);
    selected_submit(&f).await;
    idle(&f.core, &f.task).await;
    let current = f.core.bootstrap().await.unwrap().session_maps[0].clone();
    assert_eq!(current.status, SessionMapStatus::Active);
    assert_ne!(current.session_map_id, original.session_map_id);
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 3);
    assert_eq!(f.driver.reconfigure_started.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn successful_replacement_follows_durable_retirement_without_a_checkpoint_conflict() {
    let f = fixture(PromptMode::Complete, 10, 10, 10).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    let original = f.core.bootstrap().await.unwrap().session_maps[0].clone();
    selected_submit(&f).await;
    idle(&f.core, &f.task).await;
    let current = f.core.bootstrap().await.unwrap().session_maps[0].clone();
    assert_eq!(current.status, SessionMapStatus::Active);
    assert_ne!(current.session_map_id, original.session_map_id);
    assert_eq!(current.model.as_deref(), Some("changed"));
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 2);
    let events = f.core.sync_events(0, 1000).await.unwrap();
    let retired = events
        .iter()
        .find(|e| e.event_type == "session.detached")
        .unwrap();
    let replacement = events
        .iter()
        .rev()
        .find(|e| e.event_type == "session.ready")
        .unwrap();
    assert!(retired.sequence < replacement.sequence);
    assert!(!events.iter().any(|e| e.event_type == "turn.failed"));
}

#[tokio::test]
async fn failed_warm_resume_retires_the_original_cached_resource_before_retry() {
    let f = fixture(PromptMode::Complete, 10, 10, 10).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    f.driver.resume_error.store(true, Ordering::Release);
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    assert!(f.driver.handles.lock().unwrap().is_empty());
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_ne!(
        f.core.bootstrap().await.unwrap().session_maps[0].status,
        SessionMapStatus::Active
    );
    f.driver.resume_error.store(false, Ordering::Release);
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 2);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn reconfiguration_uses_the_exit_proof_captured_before_native_lookup_disappears() {
    let f = fixture(PromptMode::Complete, 10, 10, 10).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    f.driver.close_error.store(true, Ordering::Release);
    f.driver
        .reconfigure_removes_source
        .store(true, Ordering::Release);
    selected_submit(&f).await;
    wait_for(|| f.driver.proof_observed.load(Ordering::Acquire)).await;
    assert!(f.driver.handles.lock().unwrap().is_empty());
    // Absence from the native table itself cannot authorize a replacement.
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    f.core
        .execute(cancel(&f.core, &f.task).await)
        .await
        .unwrap();
    f.driver.proof.add_permits(1);
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps[0].status,
        SessionMapStatus::Detached
    );
}

#[tokio::test]
async fn a_source_without_a_local_handle_cannot_remain_active_after_replacement_rejection() {
    let f = fixture(PromptMode::Complete, 10, 10, 10).await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    // Simulate the native session already gone before configuration begins.
    f.driver.handles.lock().unwrap().clear();
    f.driver.create_rejected.store(true, Ordering::Release);
    selected_submit(&f).await;
    idle(&f.core, &f.task).await;
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps[0].status,
        SessionMapStatus::Detached
    );
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 2);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn replacement_admission_requires_a_durable_source_retirement_record() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("private.sqlite");
    let f = fixture_with_store(
        PromptMode::Complete,
        10,
        10,
        10,
        SqliteStore::open(&db).unwrap(),
    )
    .await;
    submit(&f.core, &f.task).await;
    idle(&f.core, &f.task).await;
    let database = rusqlite::Connection::open(&db).unwrap();
    database.execute_batch("CREATE TRIGGER private_retirement_failure BEFORE INSERT ON events WHEN NEW.event_type = 'session.detached' BEGIN SELECT RAISE(ABORT, 'private retirement write failure'); END;").unwrap();
    selected_submit(&f).await;
    idle(&f.core, &f.task).await;
    assert!(f.driver.handles.lock().unwrap().is_empty());
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(
        f.core.bootstrap().await.unwrap().session_maps[0].status,
        SessionMapStatus::Detached
    );
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(events.iter().any(|e| e.event_type == "turn.failed"));
    database
        .execute_batch("DROP TRIGGER private_retirement_failure;")
        .unwrap();
    selected_submit(&f).await;
    idle(&f.core, &f.task).await;
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 2);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 2);
}
