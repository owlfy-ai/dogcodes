//! Memory-only native attempts and actual Core run ownership, no installed agents.
use super::*;
use crate::{recovery::RunOwnership, run_cleanup::Execution, tests::MemoryRepository};
use agent_domain::{ProjectId, RunId, TaskId, TurnId};
use driver_api::{DriverError, DriverEventStream, FakeDriver, PromptRequest, SessionRequest};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Semaphore;

#[derive(Clone, Copy)]
enum Mode {
    Ready,
    Reject,
    Panic,
}
struct Driver {
    inner: FakeDriver,
    handle: SessionHandle,
    mode: Mode,
    create: Semaphore,
    close: Semaphore,
    creates: AtomicUsize,
    closes: AtomicUsize,
    prompts: AtomicUsize,
    alive: AtomicBool,
    close_error: bool,
    proof: Arc<Semaphore>,
}
#[async_trait::async_trait]
impl AgentDriver for Driver {
    fn executor(&self) -> &agent_domain::ExecutorRef {
        self.inner.executor()
    }
    fn driver_version(&self) -> &'static str {
        "private-startup"
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        self.creates.fetch_add(1, Ordering::AcqRel);
        self.create.acquire().await.unwrap().forget();
        match self.mode {
            Mode::Ready => {
                self.alive.store(true, Ordering::Release);
                Ok(self.handle.clone())
            }
            Mode::Reject => Err(DriverError::NotConfigured(
                "private explicit setup rejection".into(),
            )),
            Mode::Panic => {
                self.alive.store(true, Ordering::Release);
                panic!("private startup panic")
            }
        }
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        _: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        self.prompts.fetch_add(1, Ordering::AcqRel);
        panic!("startup cannot send a model prompt")
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        panic!("startup cannot interrupt a nonexistent native turn")
    }
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        assert!(handle.same_scope(&self.handle));
        self.closes.fetch_add(1, Ordering::AcqRel);
        self.close.acquire().await.unwrap().forget();
        if self.close_error {
            return Err(DriverError::Disconnected(
                "private close remains unknown".into(),
            ));
        }
        self.alive.store(false, Ordering::Release);
        Ok(())
    }
    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        assert!(handle.same_scope(&self.handle));
        let proof = self.proof.clone();
        self.close_error.then(|| {
            Box::pin(async move {
                proof.acquire().await.unwrap().forget();
            }) as driver_api::SessionShutdownProof
        })
    }
}
fn driver(mode: Mode, close_error: bool) -> Arc<Driver> {
    Arc::new(Driver {
        inner: FakeDriver::new("fake").unwrap(),
        handle: SessionHandle::scoped("vendor-reuses-this-id".into()),
        mode,
        create: Semaphore::new(0),
        close: Semaphore::new(0),
        creates: AtomicUsize::new(0),
        closes: AtomicUsize::new(0),
        prompts: AtomicUsize::new(0),
        alive: AtomicBool::new(false),
        close_error,
        proof: Arc::new(Semaphore::new(0)),
    })
}
struct Fixture {
    core: Arc<Core>,
    repository: Arc<MemoryRepository>,
    driver: Arc<Driver>,
    task: TaskId,
    turn: TurnId,
    run: RunId,
    ownership: Option<RunOwnership>,
}
async fn fixture(mode: Mode, close_error: bool) -> Fixture {
    let repository = Arc::new(MemoryRepository::default());
    // Unlike static overrides used by unrelated unit fixtures, this fixture
    // exercises real CAS writes and original-run terminal projection.
    repository
        .project_task_events
        .store(true, Ordering::Release);
    let core = Arc::new(
        Core::new(repository.clone()).with_history_query_program(std::env::current_exe().unwrap()),
    );
    let driver = driver(mode, close_error);
    let project = ProjectId::new();
    let task = TaskId::new();
    let turn = TurnId::new();
    let run = RunId::new();
    core.execute(agent_protocol::CommandEnvelope::local(
        agent_protocol::CoreCommand::RegisterProject(agent_protocol::RegisterProject {
            project_id: project.clone(),
            display_name: "private startup".into(),
            path: "/tmp".into(),
            additional_directories: vec![],
        }),
    ))
    .await
    .unwrap();
    *repository.task_override.lock().await = Some(agent_protocol::TaskSummary {
        task_id: task.clone(),
        project_id: project,
        title: "private startup".into(),
        preferred_executor: driver.executor().clone(),
        last_turn_executor: None,
        updated_at: None,
        stream_version: 0,
        active_primary_run: Some(agent_protocol::ActiveRunSummary {
            run_id: run.clone(),
            turn_id: turn.clone(),
            executor: driver.executor().clone(),
            state: agent_domain::RunState::Running,
            started_at: chrono::Utc::now(),
        }),
    });
    let ownership = Some(core.own_run(run.clone(), driver.clone()));
    Fixture {
        core,
        repository,
        driver,
        task,
        turn,
        run,
        ownership,
    }
}
async fn operation(driver: Arc<Driver>) -> Result<(SessionHandle, ()), CoreError> {
    Ok((
        driver
            .create_session(SessionRequest {
                working_directory: "/tmp".into(),
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
            })
            .await?,
        (),
    ))
}
async fn wait_for(mut test: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn start(
    f: &mut Fixture,
    history: bool,
) -> (
    Arc<Control>,
    tokio::task::JoinHandle<Result<(SessionHandle, ()), CoreError>>,
) {
    let execution = Execution::new(
        f.core.clone(),
        f.task.clone(),
        f.turn.clone(),
        f.ownership.take().unwrap(),
    );
    let control = execution.control();
    control.hold_workspace(f.core.workspace_access.clone().read_owned().await);
    let lease = if history {
        f.core
            .history_lease(&f.task, f.driver.executor())
            .await
            .unwrap()
            .map(Arc::new)
    } else {
        None
    };
    let waiter = Startup::start(
        f.core.clone(),
        control.clone(),
        f.driver.clone(),
        lease,
        operation(f.driver.clone()),
    )
    .unwrap();
    let worker = tokio::spawn(async move {
        let _execution = execution;
        waiter.result().await
    });
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 1).await;
    (control, worker)
}

#[tokio::test]
async fn abandoned_waiter_accepts_the_late_original_handle_and_waits_for_its_single_close() {
    let mut f = fixture(Mode::Ready, false).await;
    let (control, worker) = start(&mut f, true).await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(control.stopping());
    assert!(f.core.try_workspace_mutation().is_err());
    wait_for(|| control.cleanup_pending()).await;
    assert!(f.core.require_task_cleanup_clear(&f.task).is_err());
    let replacement = driver(Mode::Ready, false);
    f.core.register_driver(replacement.clone()).await;
    f.driver.create.add_permits(1);
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    assert!(f.driver.alive.load(Ordering::Acquire));
    assert!(
        f.core
            .run_session(&f.run, f.driver.executor())
            .unwrap()
            .same_scope(&f.driver.handle)
    );
    assert!(
        f.core
            .history_lease(&f.task, f.driver.executor())
            .await
            .is_err(),
        "late startup lost its live history grant"
    );
    assert_eq!(replacement.closes.load(Ordering::Acquire), 0);
    assert_eq!(f.driver.prompts.load(Ordering::Acquire), 0);
    f.driver.close.add_permits(1);
    wait_for(|| !f.core.owned_runs.lock().unwrap().contains_key(&f.run)).await;
    assert!(!f.driver.alive.load(Ordering::Acquire));
    assert!(control.finished());
    assert!(f.core.try_workspace_mutation().is_ok());
    assert!(f.core.require_task_cleanup_clear(&f.task).is_ok());
    assert!(
        f.core
            .history_lease(&f.task, f.driver.executor())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn late_setup_rejection_resolves_abandonment_without_guessing_a_handle() {
    let mut f = fixture(Mode::Reject, false).await;
    let (control, worker) = start(&mut f, true).await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    wait_for(|| control.cleanup_pending()).await;
    f.driver.create.add_permits(1);
    wait_for(|| !f.core.owned_runs.lock().unwrap().contains_key(&f.run)).await;
    assert!(control.finished());
    assert!(!control.owns_native());
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
    assert!(f.core.try_workspace_mutation().is_ok());
    assert!(
        f.core
            .history_lease(&f.task, f.driver.executor())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn panicking_setup_without_a_native_identity_remains_unknown() {
    let mut f = fixture(Mode::Panic, false).await;
    let (control, worker) = start(&mut f, true).await;
    f.driver.create.add_permits(1);
    assert!(worker.await.unwrap().is_err());
    wait_for(|| control.cleanup_pending()).await;
    assert!(f.driver.alive.load(Ordering::Acquire));
    assert!(control.owns_native());
    assert!(!control.finished());
    assert!(f.core.try_workspace_mutation().is_err());
    assert!(f.core.owned_runs.lock().unwrap().contains_key(&f.run));
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 0);
    assert!(
        f.core
            .history_lease(&f.task, f.driver.executor())
            .await
            .is_err()
    );
    assert!(
        !f.core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "turn.failed")
    );
}

#[tokio::test]
async fn an_already_accepted_stop_prevents_native_startup_registration_and_polling() {
    let mut f = fixture(Mode::Ready, false).await;
    let ownership = f.ownership.take().unwrap();
    let control = ownership.control.clone();
    control.request_stop();
    assert!(
        Startup::start(
            f.core.clone(),
            control.clone(),
            f.driver.clone(),
            None,
            operation(f.driver.clone())
        )
        .is_none()
    );
    assert!(!control.owns_native());
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 0);
    assert!(control.startup().is_none());
    control.finish();
    drop(ownership);
}

#[tokio::test]
async fn a_started_cleanup_cannot_be_replaced_by_a_new_startup_attempt() {
    let mut f = fixture(Mode::Ready, false).await;
    let ownership = f.ownership.take().unwrap();
    let control = ownership.control.clone();
    f.core.wait_run_cleanup(&control).await;
    assert!(!control.cleanup_pending());
    assert!(
        Startup::start(
            f.core.clone(),
            control.clone(),
            f.driver.clone(),
            None,
            operation(f.driver.clone())
        )
        .is_none()
    );
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 0);
    control.finish();
    drop(ownership);
}

#[tokio::test]
async fn orphan_late_success_is_retired_on_its_original_driver_even_after_run_ownership_is_gone() {
    let mut f = fixture(Mode::Ready, false).await;
    let ownership = f.ownership.take().unwrap();
    let control = ownership.control.clone();
    control.hold_workspace(f.core.workspace_access.clone().read_owned().await);
    let history = f
        .core
        .history_lease(&f.task, f.driver.executor())
        .await
        .unwrap()
        .map(Arc::new);
    let waiter = Startup::start(
        f.core.clone(),
        control.clone(),
        f.driver.clone(),
        history,
        operation(f.driver.clone()),
    )
    .unwrap();
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 1).await;
    drop(ownership);
    f.driver.create.add_permits(1);
    assert!(waiter.result().await.is_err());
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    assert!(f.driver.alive.load(Ordering::Acquire));
    assert!(f.core.try_workspace_mutation().is_err());
    f.driver.close.add_permits(1);
    wait_for(|| !f.driver.alive.load(Ordering::Acquire)).await;
    let startup = control.startup().unwrap();
    let Outcome::Closing(cleanup) = startup.resolution().await else {
        panic!("orphan must own its original cleanup");
    };
    cleanup.wait_confirmed().await;
    wait_for(|| startup.history.lock().unwrap().is_none() && control.finished()).await;
    assert!(f.core.try_workspace_mutation().is_ok());
    assert!(
        f.core
            .history_lease(&f.task, f.driver.executor())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn late_handle_cleanup_keeps_waiting_for_the_captured_exit_proof_after_a_close_error() {
    let mut f = fixture(Mode::Ready, true).await;
    let (control, worker) = start(&mut f, true).await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    f.driver.create.add_permits(1);
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    f.driver.close.add_permits(1);
    wait_for(|| control.cleanup_pending()).await;
    assert!(f.core.try_workspace_mutation().is_err());
    assert!(
        f.core
            .history_lease(&f.task, f.driver.executor())
            .await
            .is_err()
    );
    f.driver.alive.store(false, Ordering::Release);
    f.driver.proof.add_permits(1);
    wait_for(|| control.finished()).await;
    assert!(f.core.try_workspace_mutation().is_ok());
    assert_eq!(f.driver.closes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn late_cleanup_does_not_publish_failure_into_a_successor_run() {
    let mut f = fixture(Mode::Ready, false).await;
    let (_control, worker) = start(&mut f, false).await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    f.repository
        .task_override
        .lock()
        .await
        .as_mut()
        .unwrap()
        .active_primary_run
        .as_mut()
        .unwrap()
        .run_id = RunId::new();
    f.driver.create.add_permits(1);
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    f.driver.close.add_permits(1);
    wait_for(|| !f.core.owned_runs.lock().unwrap().contains_key(&f.run)).await;
    assert!(
        !f.core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "turn.failed")
    );
}

#[tokio::test]
async fn dropping_the_execution_outside_a_runtime_uses_its_captured_original_runtime() {
    let mut f = fixture(Mode::Ready, false).await;
    let execution = Execution::new(
        f.core.clone(),
        f.task.clone(),
        f.turn.clone(),
        f.ownership.take().unwrap(),
    );
    let control = execution.control();
    control.hold_workspace(f.core.workspace_access.clone().read_owned().await);
    let waiter = Startup::start(
        f.core.clone(),
        control.clone(),
        f.driver.clone(),
        None,
        operation(f.driver.clone()),
    )
    .unwrap();
    wait_for(|| f.driver.creates.load(Ordering::Acquire) == 1).await;
    drop(waiter);
    std::thread::spawn(move || drop(execution)).join().unwrap();
    // The new reaper owns the original RunOwnership before it is scheduled.
    assert!(f.core.owned_runs.lock().unwrap().contains_key(&f.run));
    assert!(f.core.try_workspace_mutation().is_err());
    f.driver.create.add_permits(1);
    wait_for(|| f.driver.closes.load(Ordering::Acquire) == 1).await;
    assert!(f.core.owned_runs.lock().unwrap().contains_key(&f.run));
    f.driver.close.add_permits(1);
    wait_for(|| !f.core.owned_runs.lock().unwrap().contains_key(&f.run)).await;
    assert!(control.finished());
    assert!(f.core.try_workspace_mutation().is_ok());
}
