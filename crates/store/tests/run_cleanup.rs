#[path = "support/installations.rs"]
mod installations;
use agent_core::{Core, CoreRepository};
use agent_domain::{ArtifactId, ExecutorRef, ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use async_trait::async_trait;
use driver_api::*;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc};

#[derive(Clone, Copy)]
enum Mode {
    Hold,
    Complete,
    ThoughtComplete,
    Fail,
    Silent,
    Panic,
}
struct Resource {
    handle: SessionHandle,
    output: Option<mpsc::UnboundedSender<Result<DriverEvent, DriverError>>>,
}
struct Driver {
    executor: ExecutorRef,
    mode: Mode,
    close: Semaphore,
    creates: AtomicUsize,
    prompts: AtomicUsize,
    closes: AtomicUsize,
    interrupts: AtomicUsize,
    retired: AtomicUsize,
    resources: Mutex<HashMap<String, Resource>>,
    autonomous: Mutex<Option<DriverEventStream>>,
    proof_enabled: std::sync::atomic::AtomicBool,
    close_error: std::sync::atomic::AtomicBool,
    proof: Arc<Semaphore>,
    directory: Arc<tempfile::TempDir>,
}
impl Driver {
    fn check(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        if self
            .resources
            .lock()
            .unwrap()
            .get(&handle.native_session_id)
            .is_some_and(|r| r.handle.same_scope(handle))
        {
            Ok(())
        } else {
            Err(DriverError::SessionIncompatible(
                "wrong original resource".into(),
            ))
        }
    }
}
#[async_trait]
impl AgentDriver for Driver {
    async fn take_autonomous_events(&self, _: &SessionHandle) -> Option<DriverEventStream> {
        self.autonomous.lock().unwrap().take()
    }
    async fn shutdown_proof(&self, handle: &SessionHandle) -> Option<SessionShutdownProof> {
        if !self.proof_enabled.load(Ordering::Acquire) || self.check(handle).is_err() {
            return None;
        }
        let proof = self.proof.clone();
        Some(Box::pin(async move {
            proof.acquire().await.unwrap().forget();
        }))
    }
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "resource-fixture"
    }
    fn supports_live_session_reuse(&self) -> bool {
        true
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        let index = self.creates.fetch_add(1, Ordering::AcqRel) + 1;
        let handle = SessionHandle::scoped(format!("native-{index}"));
        self.resources.lock().unwrap().insert(
            handle.native_session_id.clone(),
            Resource {
                handle: handle.clone(),
                output: None,
            },
        );
        Ok(handle)
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        _: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        self.check(handle)?;
        self.prompts.fetch_add(1, Ordering::AcqRel);
        let (tx, rx) = mpsc::unbounded_channel();
        match self.mode {
            Mode::Hold => {
                tx.send(Ok(DriverEvent::TextDelta {
                    item_id: "answer".into(),
                    text: "private answer".into(),
                }))
                .unwrap();
                self.resources
                    .lock()
                    .unwrap()
                    .get_mut(&handle.native_session_id)
                    .unwrap()
                    .output = Some(tx);
            }
            Mode::Panic => {
                std::fs::write(
                    self.directory.path().join("changed.txt"),
                    b"written before worker failure",
                )
                .unwrap();
                self.resources
                    .lock()
                    .unwrap()
                    .get_mut(&handle.native_session_id)
                    .unwrap()
                    .output = Some(tx);
                panic!("private worker panic after original resource admission");
            }
            Mode::Complete | Mode::ThoughtComplete => {
                if matches!(self.mode, Mode::ThoughtComplete) {
                    tx.send(Ok(DriverEvent::Presentation {
                        item_id:"fixture-thought".into(),
                        data:json!({"type":"thought","status":"running","delta":"published fixture thought"}),
                    })).unwrap();
                }
                tx.send(Ok(DriverEvent::TurnCompleted {
                    summary: "done".into(),
                }))
                .unwrap();
            }
            Mode::Fail => {
                tx.send(Ok(DriverEvent::TurnFailed {
                    reason: "private model failure".into(),
                }))
                .unwrap();
            }
            Mode::Silent => {}
        }
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|e| (e, rx))
        })))
    }
    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        self.interrupts.fetch_add(1, Ordering::AcqRel);
        self.check(handle)
    }
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        self.check(&handle)?;
        self.closes.fetch_add(1, Ordering::AcqRel);
        self.close.acquire().await.unwrap().forget();
        self.check(&handle)?;
        self.resources
            .lock()
            .unwrap()
            .remove(&handle.native_session_id);
        if self.close_error.load(Ordering::Acquire) {
            return Err(DriverError::Protocol("private close response lost".into()));
        }
        self.retired.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
struct Fixture {
    core: Arc<Core>,
    driver: Arc<Driver>,
    task: TaskId,
    root: Arc<tempfile::TempDir>,
}
fn driver(executor: ExecutorRef, mode: Mode, root: Arc<tempfile::TempDir>) -> Arc<Driver> {
    Arc::new(Driver {
        executor,
        mode,
        close: Semaphore::new(0),
        creates: AtomicUsize::new(0),
        prompts: AtomicUsize::new(0),
        closes: AtomicUsize::new(0),
        interrupts: AtomicUsize::new(0),
        retired: AtomicUsize::new(0),
        resources: Mutex::default(),
        autonomous: Mutex::default(),
        proof_enabled: std::sync::atomic::AtomicBool::new(false),
        close_error: std::sync::atomic::AtomicBool::new(false),
        proof: Arc::new(Semaphore::new(0)),
        directory: root,
    })
}
async fn fixture(mode: Mode, failure: Option<&str>) -> Fixture {
    fixture_with_workspace(mode, failure, None).await
}
async fn fixture_with_workspace(
    mode: Mode,
    failure: Option<&str>,
    workspace: Option<Arc<dyn agent_core::WorkspaceContextProvider>>,
) -> Fixture {
    let root = Arc::new(tempfile::tempdir().unwrap());
    let db = root.path().join("state.sqlite");
    let store = Arc::new(SqliteStore::open(&db).unwrap());
    let core = Arc::new(if let Some(workspace) = workspace {
        Core::with_workspace_context(store, workspace)
    } else {
        Core::new(store)
    });
    let driver = driver(
        installations::probe(&core, "fake").await,
        mode,
        root.clone(),
    );
    core.register_driver(driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "private resource fixture".into(),
            path: root.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "private run".into(),
            preferred_executor: driver.executor.clone(),
        },
    )))
    .await
    .unwrap();
    if let Some(kind) = failure {
        assert!(
            [
                "session.ready",
                "history.native_prompt_accepted",
                "message.delta",
                "turn.completed"
            ]
            .contains(&kind)
        );
        rusqlite::Connection::open(&db).unwrap().execute_batch(&format!("CREATE TRIGGER private_failure BEFORE INSERT ON events WHEN NEW.event_type = '{kind}' BEGIN SELECT RAISE(ABORT, 'private write failure'); END;")).unwrap();
    }
    Fixture {
        core,
        driver,
        task,
        root,
    }
}
async fn submit(f: &Fixture) {
    let state = f.core.bootstrap().await.unwrap();
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: f.task.clone(),
        prompt: "private task".into(),
        attachments: vec![],
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
            .find(|t| t.task_id == f.task)
            .unwrap()
            .stream_version,
    );
    f.core.execute(command).await.unwrap();
}
async fn wait_for(test: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn idle(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.core
                .bootstrap()
                .await
                .unwrap()
                .tasks
                .iter()
                .find(|t| t.task_id == f.task)
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
    .unwrap();
}

#[tokio::test]
async fn failures_before_or_after_prompt_keep_original_resources_until_confirmed_cleanup() {
    for kind in [
        "session.ready",
        "history.native_prompt_accepted",
        "message.delta",
    ] {
        let f = fixture(Mode::Hold, Some(kind)).await;
        submit(&f).await;
        wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
        assert!(
            f.core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .is_some(),
            "{kind}"
        );
        assert!(
            !f.core.sync_events(0, 1000).await.unwrap().iter().any(|e| [
                "turn.completed",
                "turn.failed"
            ]
            .contains(&e.event_type.as_str())),
            "{kind}"
        );
        let replacement = driver(f.driver.executor.clone(), Mode::Complete, f.root.clone());
        f.core.register_driver(replacement.clone()).await;
        f.driver.close.add_permits(1);
        idle(&f).await;
        assert_eq!(f.driver.retired.load(Ordering::Acquire), 1, "{kind}");
        assert_eq!(f.driver.closes.load(Ordering::Acquire), 1, "{kind}");
        assert_eq!(
            replacement.closes.load(Ordering::Acquire),
            0,
            "cleanup switched to replacement"
        );
        assert!(f.driver.resources.lock().unwrap().is_empty());
        let events = f.core.sync_events(0, 1000).await.unwrap();
        assert!(
            events.iter().any(|e| e.event_type == "turn.failed"),
            "{kind}"
        );
        assert!(
            !events.iter().any(|e| e.event_type == "turn.completed"),
            "{kind}"
        );
        // The retired scope must not remain labelled as a live reusable
        // session; a subsequent send rebuilds on the replacement installation.
        rusqlite::Connection::open(f.root.path().join("state.sqlite"))
            .unwrap()
            .execute_batch("DROP TRIGGER private_failure;")
            .unwrap();
        submit(&f).await;
        idle(&f).await;
        assert_eq!(
            replacement.prompts.load(Ordering::Acquire),
            1,
            "{kind}: could not continue after cleanup"
        );
        assert_eq!(
            replacement.closes.load(Ordering::Acquire),
            0,
            "old cleanup closed the successor"
        );
    }
}

#[tokio::test]
async fn a_panicking_worker_transfers_original_ownership_to_cleanup_before_terminal_failure() {
    let f = fixture(Mode::Panic, None).await;
    submit(&f).await;
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    assert!(
        f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
    );
    f.driver.close.add_permits(1);
    idle(&f).await;
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(events.iter().any(
        |e| e.event_type == "turn.failed" && e.payload["failureKind"] == "execution_abandoned"
    ));
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert!(f.driver.resources.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_or_unterminated_streams_force_retirement_even_when_idle_reuse_is_supported() {
    for mode in [Mode::Fail, Mode::Silent] {
        let f = fixture(mode, None).await;
        submit(&f).await;
        wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
        assert!(
            f.core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .is_some()
        );
        f.driver.close.add_permits(1);
        idle(&f).await;
        assert_eq!(f.driver.retired.load(Ordering::Acquire), 1);
        assert!(
            f.core
                .sync_events(0, 1000)
                .await
                .unwrap()
                .iter()
                .any(|e| e.event_type == "turn.failed")
        );
    }
}

#[tokio::test]
async fn bookkeeping_failure_after_successful_ownership_transfer_preserves_completion_and_idle_session()
 {
    let f = fixture(Mode::Complete, Some("turn.completed")).await;
    submit(&f).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f
            .core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .any(|event| event.event_type == "turn.usage_recorded")
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some(),
        "lost the completed result while storage was unavailable"
    );
    assert!(f.core.try_workspace_mutation().is_err());
    assert!(
        !f.core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .any(|event| event.event_type == "turn.failed")
    );
    let snapshot = f.core.bootstrap().await.unwrap();
    let task = &snapshot.tasks[0];
    let active = task.active_primary_run.as_ref().unwrap();
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: f.task.clone(),
        turn_id: active.turn_id.clone(),
        run_id: active.run_id.clone(),
    }));
    cancel.expected_stream_version = Some(task.stream_version);
    f.core.execute(cancel).await.unwrap();
    rusqlite::Connection::open(f.root.path().join("state.sqlite"))
        .unwrap()
        .execute_batch("DROP TRIGGER private_failure;")
        .unwrap();
    idle(&f).await;
    assert_eq!(
        f.driver.closes.load(Ordering::Acquire),
        0,
        "late storage error closed a transferred scope"
    );
    assert_eq!(f.driver.resources.lock().unwrap().len(), 1);
    assert!(
        f.core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "turn.completed")
    );
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(
        f.driver.interrupts.load(Ordering::Acquire),
        0,
        "interrupted the handed-off idle session"
    );
    let events = f.core.sync_events(0, 1000).await.unwrap();
    let completed = events
        .iter()
        .find(|e| e.event_type == "turn.completed")
        .unwrap();
    assert!(matches!(completed.source, EventSource::Driver { .. }));
}

struct RecoveringWorkspace {
    calls: AtomicUsize,
    finishes: Arc<AtomicUsize>,
}

#[tokio::test]
async fn completion_retries_the_frozen_workspace_result_without_recapturing_or_reprompting() {
    let provider = Arc::new(RecoveringWorkspace {
        calls: AtomicUsize::new(0),
        finishes: Arc::new(AtomicUsize::new(0)),
    });
    let f = fixture_with_workspace(Mode::Complete, None, Some(provider.clone())).await;
    std::fs::write(
        f.root.path().join("changed.txt"),
        b"frozen completion result",
    )
    .unwrap();
    let connection = rusqlite::Connection::open(f.root.path().join("state.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER completion_changes_failure BEFORE INSERT ON events WHEN NEW.event_type='workspace.turn_changes_captured' BEGIN SELECT RAISE(ABORT, 'private completed changes failure'); END;").unwrap();
    submit(&f).await;
    wait_for(|| provider.finishes.load(Ordering::Acquire) == 1).await;
    assert!(
        f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
    );
    assert!(f.core.try_workspace_mutation().is_err());
    std::fs::write(f.root.path().join("changed.txt"), b"later external edit").unwrap();
    connection
        .execute_batch("DROP TRIGGER completion_changes_failure;")
        .unwrap();
    idle(&f).await;
    let events = f.core.sync_events(0, 1000).await.unwrap();
    let changes = events
        .iter()
        .find(|e| e.event_type == "workspace.turn_changes_captured")
        .unwrap();
    let completed = events
        .iter()
        .find(|e| e.event_type == "turn.completed")
        .unwrap();
    assert!(changes.sequence < completed.sequence);
    assert!(!events.iter().any(|e| e.event_type == "turn.failed"));
    let store = SqliteStore::open(f.root.path().join("state.sqlite")).unwrap();
    let id =
        ArtifactId::parse(changes.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
    let bytes = store.artifact(&id).await.unwrap().unwrap().content;
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        result["files"][0]["after"]["hex"],
        hex::encode(b"frozen completion result")
    );
    assert_eq!(provider.finishes.load(Ordering::Acquire), 1);
    assert_eq!(provider.calls.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
    assert!(f.core.try_workspace_mutation().is_ok());
}

#[tokio::test]
async fn oversized_completion_changes_record_a_diagnostic_instead_of_retrying_forever() {
    let provider = Arc::new(RecoveringWorkspace {
        calls: AtomicUsize::new(0),
        finishes: Arc::new(AtomicUsize::new(0)),
    });
    let f = fixture_with_workspace(Mode::Complete, None, Some(provider.clone())).await;
    std::fs::write(f.root.path().join("changed.txt"), vec![b'x'; 600_000]).unwrap();
    submit(&f).await;
    idle(&f).await;
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(events.iter().any(|e| e.event_type == "turn.completed"));
    assert!(!events.iter().any(|e| e.event_type == "turn.failed"));
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "workspace.turn_changes_failed"
                && e.payload["failureKind"] == "output_truncated")
    );
    assert_eq!(provider.finishes.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
}
struct Capture(String, Arc<AtomicUsize>);
#[async_trait]
impl agent_core::WorkspaceTurnCapture for Capture {
    async fn checkpoint(&self) -> Result<Option<serde_json::Value>, WorkspaceCollectionDiagnostic> {
        let before = std::fs::read(std::path::Path::new(&self.0).join("changed.txt"))
            .ok()
            .map(|bytes| json!({"hex":hex::encode(bytes)}));
        Ok(Some(
            serde_json::json!({"schemaVersion":1,"root":self.0,"before":before}),
        ))
    }
    async fn finish(&self) -> Result<serde_json::Value, WorkspaceCollectionDiagnostic> {
        self.1.fetch_add(1, Ordering::AcqRel);
        let files: Vec<_> = std::fs::read(std::path::Path::new(&self.0).join("changed.txt"))
            .ok().into_iter().map(|bytes| json!({"path":"changed.txt","status":"M","after":{"hex":hex::encode(bytes)}})).collect();
        Ok(serde_json::json!({"files":files,"patch":""}))
    }
}
#[async_trait]
impl agent_core::WorkspaceContextProvider for RecoveringWorkspace {
    async fn collect(&self, _: &str) -> agent_core::WorkspaceContextSnapshot {
        agent_core::WorkspaceContextSnapshot::default()
    }
    async fn begin_turn(
        &self,
        path: &str,
    ) -> Result<Box<dyn agent_core::WorkspaceTurnCapture>, WorkspaceCollectionDiagnostic> {
        Ok(Box::new(Capture(path.into(), self.finishes.clone())))
    }
    async fn recover_turn(
        &self,
        path: &str,
        checkpoint: serde_json::Value,
    ) -> Result<serde_json::Value, WorkspaceCollectionDiagnostic> {
        assert_eq!(checkpoint["root"], path);
        self.calls.fetch_add(1, Ordering::AcqRel);
        let content = std::fs::read(std::path::Path::new(path).join("changed.txt")).unwrap();
        Ok(
            serde_json::json!({"files":[{"path":"changed.txt","status":"M","before":checkpoint["before"],"after":{"hex":hex::encode(content)}}],"patch":"private recovered patch"}),
        )
    }
}

#[tokio::test]
async fn an_abandoned_worker_recovers_its_durable_workspace_checkpoint_after_native_retirement() {
    let provider = Arc::new(RecoveringWorkspace {
        calls: AtomicUsize::new(0),
        finishes: Arc::new(AtomicUsize::new(0)),
    });
    let f = fixture_with_workspace(Mode::Panic, None, Some(provider.clone())).await;
    submit(&f).await;
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    assert_eq!(
        provider.calls.load(Ordering::Acquire),
        0,
        "recovered while original work might still write"
    );
    assert!(
        f.core.try_workspace_mutation().is_err(),
        "abandonment lost original workspace protection"
    );
    f.driver.close.add_permits(1);
    idle(&f).await;
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.retired.load(Ordering::Acquire), 1);
    let events = f.core.sync_events(0, 1000).await.unwrap();
    let changes = events
        .iter()
        .find(|e| e.event_type == "workspace.turn_changes_captured")
        .expect("lost abandoned turn changes");
    let failed = events
        .iter()
        .find(|e| e.event_type == "turn.failed")
        .unwrap();
    assert!(changes.sequence < failed.sequence);
    assert!(f.core.try_workspace_mutation().is_ok());
}

#[tokio::test]
async fn abandoned_worker_resumes_workspace_bookkeeping_after_sqlite_write_failure() {
    let provider = Arc::new(RecoveringWorkspace {
        calls: AtomicUsize::new(0),
        finishes: Arc::new(AtomicUsize::new(0)),
    });
    let f = fixture_with_workspace(Mode::Panic, None, Some(provider.clone())).await;
    let connection = rusqlite::Connection::open(f.root.path().join("state.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER recovery_write_failure BEFORE INSERT ON events WHEN NEW.event_type='workspace.turn_changes_captured' BEGIN SELECT RAISE(ABORT, 'private recovery write failure'); END;").unwrap();
    submit(&f).await;
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    f.driver.close.add_permits(1);
    // The second attempt proves the first rejected SQLite transaction did not
    // discard the owner or falsely publish a terminal event.
    wait_for(|| provider.calls.load(Ordering::Acquire) >= 2).await;
    assert!(
        f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
    );
    assert!(f.core.try_workspace_mutation().is_err());
    assert!(
        !f.core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "turn.failed")
    );
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    connection
        .execute_batch("DROP TRIGGER recovery_write_failure;")
        .unwrap();
    idle(&f).await;
    let events = f.core.sync_events(0, 1000).await.unwrap();
    let changes: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "workspace.turn_changes_captured")
        .collect();
    let failed: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "turn.failed")
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(failed.len(), 1);
    assert!(changes[0].sequence < failed[0].sequence);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert!(f.core.try_workspace_mutation().is_ok());
}

async fn autonomous_fixture(
    panic_stream: bool,
) -> (
    Fixture,
    mpsc::UnboundedSender<Result<DriverEvent, DriverError>>,
) {
    autonomous_fixture_with_workspace(panic_stream, None).await
}

async fn autonomous_fixture_with_workspace(
    panic_stream: bool,
    workspace: Option<Arc<dyn agent_core::WorkspaceContextProvider>>,
) -> (
    Fixture,
    mpsc::UnboundedSender<Result<DriverEvent, DriverError>>,
) {
    let f = fixture_with_workspace(Mode::Complete, None, workspace).await;
    let (tx, rx) = mpsc::unbounded_channel();
    *f.driver.autonomous.lock().unwrap() = Some(Box::pin(futures::stream::unfold(
        rx,
        move |mut rx| async move {
            let event = rx.recv().await?;
            if panic_stream
                && matches!(&event, Ok(DriverEvent::TextDelta { text, .. }) if text == "panic fixture")
            {
                panic!("private autonomous stream panic");
            }
            Some((event, rx))
        },
    )));
    submit(&f).await;
    idle(&f).await;
    (f, tx)
}

async fn start_autonomous(
    f: &Fixture,
    tx: &mpsc::UnboundedSender<Result<DriverEvent, DriverError>>,
) -> ActiveRunSummary {
    tx.send(Ok(DriverEvent::TurnStarted {
        native_turn_id: Some("private-auto".into()),
    }))
    .unwrap();
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "auto-answer".into(),
        text: "partial autonomous output".into(),
    }))
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = f.core.bootstrap().await.unwrap();
            let run = state.tasks[0].active_primary_run.clone();
            if let Some(run) = run
                && f.core
                    .timeline(&f.task)
                    .await
                    .unwrap()
                    .iter()
                    .any(|item| item.text.contains("partial autonomous output"))
            {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn autonomous_failures_keep_the_original_run_until_retirement() {
    for failure in ["terminal", "transport", "eof"] {
        let (f, tx) = autonomous_fixture(false).await;
        let run = start_autonomous(&f, &tx).await;
        match failure {
            "terminal" => tx
                .send(Ok(DriverEvent::TurnFailed {
                    reason: "private failure".into(),
                }))
                .unwrap(),
            "transport" => tx
                .send(Err(DriverError::Protocol("private lost stream".into())))
                .unwrap(),
            _ => {}
        }
        drop(tx);
        wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
        assert_eq!(
            f.core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .as_ref()
                .map(|r| &r.run_id),
            Some(&run.run_id),
            "{failure}: ended before native retirement"
        );
        assert!(f.core.try_workspace_mutation().is_err());
        f.driver.close.add_permits(1);
        idle(&f).await;
        assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
        assert!(f.core.sync_events(0, 1000).await.unwrap().iter().any(|e| e.event_type == "turn.failed" && e.payload["turnId"] == json!(run.turn_id)));
        assert!(f.core.try_workspace_mutation().is_ok());
    }
}

#[tokio::test]
async fn autonomous_panic_keeps_partial_output_and_uses_original_cleanup() {
    let (f, tx) = autonomous_fixture(true).await;
    let run = start_autonomous(&f, &tx).await;
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "auto-answer".into(),
        text: "panic fixture".into(),
    }))
    .unwrap();
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    assert_eq!(
        f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .as_ref()
            .map(|r| &r.run_id),
        Some(&run.run_id)
    );
    f.driver.close.add_permits(1);
    idle(&f).await;
    assert!(
        f.core
            .timeline(&f.task)
            .await
            .unwrap()
            .iter()
            .any(|item| item.text.contains("partial autonomous output"))
    );
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn autonomous_storage_failure_waits_for_captured_proof_after_original_table_entry_is_removed()
{
    let (f, tx) = autonomous_fixture(false).await;
    f.driver.proof_enabled.store(true, Ordering::Release);
    f.driver.close_error.store(true, Ordering::Release);
    let run = start_autonomous(&f, &tx).await;
    rusqlite::Connection::open(f.root.path().join("state.sqlite")).unwrap()
        .execute_batch("CREATE TRIGGER auto_failure BEFORE INSERT ON events WHEN NEW.event_type='message.delta' BEGIN SELECT RAISE(ABORT, 'private auto write failure'); END;").unwrap();
    let replacement = driver(f.driver.executor.clone(), Mode::Complete, f.root.clone());
    f.core.register_driver(replacement.clone()).await;
    f.driver.close.add_permits(1);
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "auto-answer".into(),
        text: "failed write".into(),
    }))
    .unwrap();
    wait_for(|| f.driver.resources.lock().unwrap().is_empty()).await;
    assert_eq!(
        f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .as_ref()
            .map(|r| &r.run_id),
        Some(&run.run_id)
    );
    assert!(f.core.try_workspace_mutation().is_err());
    f.driver.proof.add_permits(1);
    idle(&f).await;
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(replacement.closes.load(Ordering::Acquire), 0);
    assert!(f.core.try_workspace_mutation().is_ok());
}

#[tokio::test]
async fn autonomous_terminal_bookkeeping_failure_preserves_completion_and_future_continuations() {
    let (f, tx) = autonomous_fixture(false).await;
    let run = start_autonomous(&f, &tx).await;
    let connection = rusqlite::Connection::open(f.root.path().join("state.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER auto_terminal_failure BEFORE INSERT ON events WHEN NEW.event_type='turn.completed' BEGIN SELECT RAISE(ABORT, 'private auto terminal failure'); END;").unwrap();
    tx.send(Ok(DriverEvent::TurnCompleted {
        summary: "done".into(),
    }))
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f
            .core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .any(|event| {
                event.event_type == "turn.usage_recorded"
                    && event.payload["turnId"] == json!(run.turn_id)
            })
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .as_ref()
            .map(|r| &r.run_id),
        Some(&run.run_id)
    );
    assert!(f.core.try_workspace_mutation().is_err());
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
    connection
        .execute_batch("DROP TRIGGER auto_terminal_failure;")
        .unwrap();
    idle(&f).await;
    let events = f.core.sync_events(0, 1000).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "turn.completed" && e.payload["turnId"] == json!(run.turn_id))
    );
    assert!(
        !events
            .iter()
            .any(|e| e.event_type == "turn.failed" && e.payload["turnId"] == json!(run.turn_id))
    );
    // The same observer must still accept later roots; no replacement prompt.
    let later = start_autonomous(&f, &tx).await;
    assert_ne!(later.run_id, run.run_id);
    tx.send(Ok(DriverEvent::TurnCompleted {
        summary: "later continuation".into(),
    }))
    .unwrap();
    idle(&f).await;
    assert!(
        f.core.sync_events(0, 1000).await.unwrap().iter().any(
            |e| e.event_type == "turn.completed" && e.payload["turnId"] == json!(later.turn_id)
        )
    );
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn completed_thought_metadata_survives_write_failure_without_closing_a_successful_session() {
    for autonomous in [false, true] {
        for fault in ["activity.updated", "turn.usage_recorded"] {
            let (f, tx) = if autonomous {
                let (f, tx) = autonomous_fixture(false).await;
                start_autonomous(&f, &tx).await;
                (f, Some(tx))
            } else {
                (fixture(Mode::ThoughtComplete, None).await, None)
            };
            let connection =
                rusqlite::Connection::open(f.root.path().join("state.sqlite")).unwrap();
            let filter = if fault == "activity.updated" {
                "AND json_extract(NEW.payload_json,'$.data.type')='thought' AND json_extract(NEW.payload_json,'$.data.status')='completed'"
            } else {
                ""
            };
            connection.execute_batch(&format!("CREATE TRIGGER thought_completion_failure BEFORE INSERT ON events WHEN NEW.event_type='{fault}' {filter} BEGIN SELECT RAISE(ABORT, 'private thought metadata failure'); END;")).unwrap();
            if let Some(tx) = tx.as_ref() {
                tx.send(Ok(DriverEvent::Presentation {
                    item_id:"fixture-thought".into(),
                    data:json!({"type":"thought","status":"running","delta":"unpersisted auto thought delta"}),
                })).unwrap();
                tx.send(Ok(DriverEvent::TurnCompleted {
                    summary: "done".into(),
                }))
                .unwrap();
            } else {
                submit(&f).await;
            }
            tokio::time::timeout(Duration::from_secs(5), async {
                while !f
                    .core
                    .sync_events(0, 1000)
                    .await
                    .unwrap()
                    .iter()
                    .any(|event| {
                        event.event_type == "activity.updated"
                            && event.payload["data"]["type"] == "thought"
                            && event.payload["data"]["status"] == "running"
                    })
                {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert!(
                f.core.bootstrap().await.unwrap().tasks[0]
                    .active_primary_run
                    .is_some()
            );
            assert_eq!(
                f.driver.closes.load(Ordering::Acquire),
                0,
                "{autonomous}/{fault}: bookkeeping closed successful native work"
            );
            assert!(f.core.try_workspace_mutation().is_err());
            connection
                .execute_batch("DROP TRIGGER thought_completion_failure;")
                .unwrap();
            idle(&f).await;
            let events = f.core.sync_events(0, 1000).await.unwrap();
            let started = events
                .iter()
                .find(|e| {
                    e.event_type == "activity.updated"
                        && e.payload["data"]["type"] == "thought"
                        && e.payload["data"]["status"] == "running"
                })
                .unwrap();
            let closed = events
                .iter()
                .find(|e| {
                    e.event_type == "activity.updated"
                        && e.payload["data"]["type"] == "thought"
                        && e.payload["data"]["status"] == "completed"
                })
                .unwrap();
            let terminal = events
                .iter()
                .find(|e| {
                    e.event_type == "turn.completed"
                        && e.payload["turnId"] == closed.payload["turnId"]
                })
                .unwrap();
            let observed: chrono::DateTime<chrono::Utc> =
                serde_json::from_value(terminal.payload["completedAt"].clone()).unwrap();
            assert!(closed.sequence < terminal.sequence);
            assert!(
                closed.payload["data"]["durationMs"].as_i64().unwrap()
                    <= (observed - started.occurred_at).num_milliseconds() + 50,
                "{autonomous}/{fault}: local storage wait inflated thinking duration"
            );
            assert!(
                events
                    .iter()
                    .filter(|e| e.event_type == "activity.updated"
                        && e.payload["data"]["type"] == "thought")
                    .all(|e| e.payload["data"].get("delta").is_none())
            );
            if autonomous {
                assert!(
                    !serde_json::to_string(&events)
                        .unwrap()
                        .contains("unpersisted auto thought delta")
                );
            }
            assert!(!events.iter().any(|e| e.event_type == "turn.failed"));
            assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
            assert_eq!(f.driver.prompts.load(Ordering::Acquire), 1);
        }
    }
}

#[tokio::test]
async fn autonomous_panic_recovers_its_own_durable_workspace_baseline_after_retirement() {
    let provider = Arc::new(RecoveringWorkspace {
        calls: AtomicUsize::new(0),
        finishes: Arc::new(AtomicUsize::new(0)),
    });
    let (f, tx) = autonomous_fixture_with_workspace(true, Some(provider.clone())).await;
    // The user turn has already captured a different (empty) baseline.
    std::fs::write(f.root.path().join("changed.txt"), b"before continuation").unwrap();
    let run = start_autonomous(&f, &tx).await;
    std::fs::write(f.root.path().join("changed.txt"), b"after continuation").unwrap();
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "auto-answer".into(),
        text: "panic fixture".into(),
    }))
    .unwrap();
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    assert_eq!(provider.calls.load(Ordering::Acquire), 0);
    assert!(f.core.try_workspace_mutation().is_err());
    f.driver.close.add_permits(1);
    idle(&f).await;
    assert_eq!(provider.calls.load(Ordering::Acquire), 1);

    let events = f.core.sync_events(0, 1000).await.unwrap();
    let event = |kind| {
        events
            .iter()
            .find(|e| e.event_type == kind && e.payload["turnId"] == json!(run.turn_id))
            .unwrap_or_else(|| panic!("missing autonomous {kind}"))
    };
    let checkpoint = event("workspace.turn_checkpoint_captured");
    let changes = event("workspace.turn_changes_captured");
    assert!(checkpoint.sequence < changes.sequence);
    assert!(changes.sequence < event("turn.failed").sequence);
    // Read persisted bytes through a fresh store, rather than the live capture.
    let store = SqliteStore::open(f.root.path().join("state.sqlite")).unwrap();
    let id =
        ArtifactId::parse(changes.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
    let bytes = store.artifact(&id).await.unwrap().unwrap().content;
    let change: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        change["files"][0]["before"]["hex"],
        hex::encode(b"before continuation")
    );
    assert_eq!(
        change["files"][0]["after"]["hex"],
        hex::encode(b"after continuation")
    );
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert!(f.core.try_workspace_mutation().is_ok());
}

#[tokio::test]
async fn autonomous_git_checkpoint_recovers_unstaged_baseline_without_changing_user_index() {
    let (f, tx) = autonomous_fixture_with_workspace(
        true,
        Some(Arc::new(
            agent_workspace::GitWorkspaceContextProvider::default(),
        )),
    )
    .await;
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(f.root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--quiet"]);
    std::fs::write(f.root.path().join(".gitignore"), b"state.sqlite*\n").unwrap();
    std::fs::write(f.root.path().join("changed.txt"), b"user staged content\n").unwrap();
    git(&["add", "changed.txt", ".gitignore"]);
    let index_before = std::fs::read(f.root.path().join(".git/index")).unwrap();
    std::fs::write(f.root.path().join("changed.txt"), b"before continuation\n").unwrap();
    let run = start_autonomous(&f, &tx).await;
    std::fs::write(f.root.path().join("changed.txt"), b"after continuation\n").unwrap();
    tx.send(Ok(DriverEvent::TextDelta {
        item_id: "auto-answer".into(),
        text: "panic fixture".into(),
    }))
    .unwrap();
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    assert!(f.core.try_workspace_mutation().is_err());
    f.driver.close.add_permits(1);
    idle(&f).await;
    let events = f.core.sync_events(0, 1000).await.unwrap();
    let changes = events
        .iter()
        .find(|e| {
            e.event_type == "workspace.turn_changes_captured"
                && e.payload["turnId"] == json!(run.turn_id)
        })
        .expect("lost autonomous Git changes");
    let store = SqliteStore::open(f.root.path().join("state.sqlite")).unwrap();
    let id =
        ArtifactId::parse(changes.payload["artifact"]["artifactId"].as_str().unwrap()).unwrap();
    let bytes = store.artifact(&id).await.unwrap().unwrap().content;
    let change: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(change["attribution"], "workspace_recovery_interval");
    let files = change["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "changed.txt");
    assert_eq!(
        files[0]["before"]["hex"],
        hex::encode(b"before continuation\n")
    );
    assert_eq!(
        files[0]["after"]["hex"],
        hex::encode(b"after continuation\n")
    );
    assert_eq!(
        std::fs::read(f.root.path().join(".git/index")).unwrap(),
        index_before
    );
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
    assert!(f.core.try_workspace_mutation().is_ok());
}
