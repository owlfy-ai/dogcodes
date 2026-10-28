use super::*;
use crate::tests::MemoryRepository;
use agent_domain::{ProjectId, RunId};
use driver_api::{DriverError, FakeDriver, PromptRequest, SessionRequest};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Semaphore;

struct Driver {
    inner: FakeDriver,
    handle: SessionHandle,
    close: Semaphore,
    calls: AtomicUsize,
    fail: bool,
    proof: Option<Arc<Semaphore>>,
    proof_available: AtomicBool,
    captures: AtomicUsize,
    retired: Arc<AtomicBool>,
    api_returned: AtomicBool,
}
fn driver(fail: bool, proof: bool) -> Arc<Driver> {
    Arc::new(Driver {
        inner: FakeDriver::new("fake").unwrap(),
        handle: SessionHandle::scoped("reusable-id".into()),
        close: Semaphore::new(0),
        calls: AtomicUsize::new(0),
        fail,
        proof: proof.then(|| Arc::new(Semaphore::new(0))),
        proof_available: AtomicBool::new(true),
        captures: AtomicUsize::new(0),
        retired: Arc::new(AtomicBool::new(false)),
        api_returned: AtomicBool::new(false),
    })
}
#[async_trait::async_trait]
impl AgentDriver for Driver {
    fn executor(&self) -> &agent_domain::ExecutorRef {
        self.inner.executor()
    }
    fn driver_version(&self) -> &'static str {
        "owned-cleanup-fixture"
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        Ok(self.handle.clone())
    }
    async fn resume_session(&self, _: &str, _: &str) -> Result<SessionHandle, DriverError> {
        Ok(self.handle.clone())
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<driver_api::DriverEventStream, DriverError> {
        self.inner.prompt(handle, request).await
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        assert!(self.handle.same_scope(handle));
        self.captures.fetch_add(1, Ordering::AcqRel);
        if !self.proof_available.load(Ordering::Acquire) {
            return None;
        }
        let proof = self.proof.clone()?;
        let retired = self.retired.clone();
        Some(Box::pin(async move {
            proof.acquire().await.unwrap().forget();
            retired.store(true, Ordering::Release);
        }))
    }
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        assert!(self.handle.same_scope(&handle));
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.close.acquire().await.unwrap().forget();
        self.api_returned.store(true, Ordering::Release);
        if self.fail {
            return Err(DriverError::Disconnected(
                "original close is still unconfirmed".into(),
            ));
        }
        self.retired.store(true, Ordering::Release);
        Ok(())
    }
}
async fn status(cleanup: &Cleanup, expected: Status) {
    let mut changes = cleanup.subscribe();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if *changes.borrow_and_update() == expected {
                return;
            }
            changes.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
async fn wait_for(test: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_resource_carries_its_original_exit_proof_across_native_reconfiguration() {
    let driver = driver(true, true);
    let resource = Resource::capture(driver.clone(), driver.handle.clone()).await;
    driver.proof_available.store(false, Ordering::Release);
    let cleanup = resource.retire(Duration::from_millis(20));
    status(&cleanup, Status::Pending).await;
    assert_eq!(driver.captures.load(Ordering::Acquire), 1);
    assert_eq!(driver.calls.load(Ordering::Acquire), 1);
    assert!(!driver.api_returned.load(Ordering::Acquire));
    driver.proof.as_ref().unwrap().add_permits(1);
    status(&cleanup, Status::Confirmed).await;
    // The pending original close is neither cancelled nor replayed.
    driver.close.add_permits(1);
    wait_for(|| driver.api_returned.load(Ordering::Acquire)).await;
    assert!(cleanup.confirmed());
    assert_eq!(driver.captures.load(Ordering::Acquire), 1);
    assert_eq!(driver.calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn dropping_an_unused_source_snapshot_cannot_close_a_successful_warm_session() {
    let driver = driver(false, true);
    let resource = Resource::capture(driver.clone(), driver.handle.clone()).await;
    drop(resource);
    tokio::task::yield_now().await;
    assert_eq!(driver.captures.load(Ordering::Acquire), 1);
    assert_eq!(driver.calls.load(Ordering::Acquire), 0);
    assert!(!driver.retired.load(Ordering::Acquire));
}

#[tokio::test]
async fn deadline_and_dropped_observers_do_not_drop_or_repeat_the_original_shutdown() {
    let driver = driver(false, false);
    let cleanup = Cleanup::start(
        Some(driver.clone()),
        Some(driver.handle.clone()),
        true,
        Duration::from_millis(20),
    );
    status(&cleanup, Status::Pending).await;
    drop(cleanup.subscribe());
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(driver.calls.load(Ordering::Acquire), 1);
    assert!(!driver.retired.load(Ordering::Acquire));
    driver.close.add_permits(1);
    status(&cleanup, Status::Confirmed).await;
    assert!(driver.retired.load(Ordering::Acquire));
    assert_eq!(driver.calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_failed_close_stays_owned_until_the_captured_positive_proof_without_retry() {
    let driver = driver(true, true);
    driver.close.add_permits(1);
    let cleanup = Cleanup::start(
        Some(driver.clone()),
        Some(driver.handle.clone()),
        true,
        Duration::from_millis(20),
    );
    status(&cleanup, Status::Pending).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(driver.calls.load(Ordering::Acquire), 1);
    assert_eq!(driver.captures.load(Ordering::Acquire), 1);
    assert!(!driver.retired.load(Ordering::Acquire));
    driver.proof.as_ref().unwrap().add_permits(1);
    status(&cleanup, Status::Confirmed).await;
    assert_eq!(driver.calls.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn failure_without_passive_proof_is_unknown_and_never_becomes_a_fake_success() {
    let driver = driver(true, false);
    driver.close.add_permits(1);
    let cleanup = Cleanup::start(
        Some(driver.clone()),
        Some(driver.handle.clone()),
        true,
        Duration::from_millis(20),
    );
    status(&cleanup, Status::Pending).await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(60),
            status(&cleanup, Status::Confirmed)
        )
        .await
        .is_err()
    );
    assert_eq!(driver.calls.load(Ordering::Acquire), 1);
    assert!(!cleanup.confirmed());
    assert!(!driver.retired.load(Ordering::Acquire));
}

#[tokio::test]
async fn aborting_a_cleanup_waiter_keeps_the_run_lease_and_never_targets_registry_replacement_or_new_run()
 {
    let repository = Arc::new(MemoryRepository::default());
    let core = Arc::new(Core::new(repository.clone()));
    let project = ProjectId::new();
    let task = TaskId::new();
    let run = RunId::new();
    let turn = TurnId::new();
    core.execute(agent_protocol::CommandEnvelope::local(
        agent_protocol::CoreCommand::RegisterProject(agent_protocol::RegisterProject {
            project_id: project.clone(),
            display_name: "private run cleanup".into(),
            path: "/tmp".into(),
            additional_directories: vec![],
        }),
    ))
    .await
    .unwrap();
    let concrete = driver(false, false);
    let original: Arc<dyn AgentDriver> = concrete.clone();
    *repository.task_override.lock().await = Some(agent_protocol::TaskSummary {
        task_id: task.clone(),
        project_id: project,
        title: "private original run".into(),
        preferred_executor: original.executor().clone(),
        last_turn_executor: None,
        updated_at: None,
        stream_version: 0,
        active_primary_run: Some(agent_protocol::ActiveRunSummary {
            run_id: run.clone(),
            turn_id: turn.clone(),
            executor: original.executor().clone(),
            state: agent_domain::RunState::Running,
            started_at: chrono::Utc::now(),
        }),
    });
    let ownership = core.own_run(run.clone(), original.clone());
    core.bind_run_session(&run, &original, concrete.handle.clone())
        .unwrap();
    let execution = Execution::new(core.clone(), task.clone(), turn, ownership);
    let control = execution.control();
    control.hold_workspace(core.workspace_access.clone().read_owned().await);
    control.session_starting();
    let waiting = tokio::spawn({
        let core = core.clone();
        let control = control.clone();
        async move {
            let _execution = execution;
            core.wait_run_cleanup(&control).await;
        }
    });
    wait_for(|| concrete.calls.load(Ordering::Acquire) == 1).await;
    waiting.abort();
    let _ = waiting.await;
    assert!(core.owned_runs.lock().unwrap().contains_key(&run));
    assert!(
        core.try_workspace_mutation().is_err(),
        "aborted execution released workspace protection before cleanup"
    );
    assert!(core.require_task_cleanup_clear(&task).is_err());
    let replacement = driver(false, false);
    core.register_driver(replacement.clone()).await;
    // Even if a newer canonical run has appeared, late cleanup may only end
    // its original resources, never cancel or fail that successor.
    repository
        .task_override
        .lock()
        .await
        .as_mut()
        .unwrap()
        .active_primary_run
        .as_mut()
        .unwrap()
        .run_id = RunId::new();
    concrete.close.add_permits(1);
    wait_for(|| !core.owned_runs.lock().unwrap().contains_key(&run)).await;
    assert!(control.finished());
    assert!(core.try_workspace_mutation().is_ok());
    assert!(core.require_task_cleanup_clear(&task).is_ok());
    assert_eq!(concrete.calls.load(Ordering::Acquire), 1);
    assert_eq!(replacement.calls.load(Ordering::Acquire), 0);
    assert!(
        !core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "turn.failed")
    );
}

#[tokio::test]
async fn positive_original_exit_does_not_wait_for_or_cancel_a_stuck_shutdown_response() {
    let driver = driver(true, true);
    let cleanup = Cleanup::start(
        Some(driver.clone()),
        Some(driver.handle.clone()),
        true,
        Duration::from_millis(20),
    );
    wait_for(|| driver.calls.load(Ordering::Acquire) == 1).await;
    driver.proof.as_ref().unwrap().add_permits(1);
    status(&cleanup, Status::Confirmed).await;
    assert!(driver.retired.load(Ordering::Acquire));
    assert!(!driver.api_returned.load(Ordering::Acquire));
    driver.close.add_permits(1);
    wait_for(|| driver.api_returned.load(Ordering::Acquire)).await;
    assert!(
        cleanup.confirmed(),
        "late RPC error cannot revoke original physical exit"
    );
    assert_eq!(driver.calls.load(Ordering::Acquire), 1);
    assert_eq!(driver.captures.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn abandoned_run_keeps_ownership_until_workspace_and_terminal_bookkeeping_are_durable() {
    for kind in ["workspace.turn_changes_failed", "turn.failed"] {
        let repository = Arc::new(MemoryRepository::default());
        repository
            .project_task_events
            .store(true, Ordering::Release);
        *repository.unavailable_event.lock().await = Some(kind.into());
        let core = Arc::new(Core::new(repository.clone()));
        let concrete = driver(false, false);
        let original: Arc<dyn AgentDriver> = concrete.clone();
        let task = TaskId::new();
        let run = RunId::new();
        let turn = TurnId::new();
        *repository.task_override.lock().await = Some(agent_protocol::TaskSummary {
            task_id: task.clone(),
            project_id: ProjectId::new(),
            title: "private storage failure".into(),
            preferred_executor: original.executor().clone(),
            last_turn_executor: None,
            updated_at: None,
            stream_version: 0,
            active_primary_run: Some(agent_protocol::ActiveRunSummary {
                run_id: run.clone(),
                turn_id: turn.clone(),
                executor: original.executor().clone(),
                state: agent_domain::RunState::Running,
                started_at: chrono::Utc::now(),
            }),
        });
        let ownership = core.own_run(run.clone(), original.clone());
        core.bind_run_session(&run, &original, concrete.handle.clone())
            .unwrap();
        let execution = Execution::new(core.clone(), task.clone(), turn.clone(), ownership);
        let control = execution.control();
        control.hold_workspace(core.workspace_access.clone().read_owned().await);
        control.session_starting();
        concrete.close.add_permits(1);
        drop(execution);
        wait_for(|| repository.rejected_events.load(Ordering::Acquire) > 0).await;
        tokio::task::yield_now().await;
        assert!(
            core.owned_runs.lock().unwrap().contains_key(&run),
            "{kind}: lost finalization owner"
        );
        assert!(!control.finished());
        assert!(core.try_workspace_mutation().is_err());
        assert!(
            !core
                .sync_events(0, 1000)
                .await
                .unwrap()
                .iter()
                .any(|e| e.event_type == "turn.failed")
        );
        let replacement = driver(false, false);
        core.register_driver(replacement.clone()).await;
        *repository.unavailable_event.lock().await = None;
        wait_for(|| !core.owned_runs.lock().unwrap().contains_key(&run)).await;
        assert!(control.finished());
        assert!(core.try_workspace_mutation().is_ok());
        assert_eq!(concrete.calls.load(Ordering::Acquire), 1);
        assert_eq!(replacement.calls.load(Ordering::Acquire), 0);
        let events = core.sync_events(0, 1000).await.unwrap();
        let failed: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "turn.failed")
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].payload["turnId"], serde_json::json!(turn));
        let workspace = events
            .iter()
            .find(|e| e.event_type == "workspace.turn_changes_failed")
            .unwrap();
        assert!(workspace.sequence < failed[0].sequence);
        assert!(
            core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .is_none()
        );
    }
}
