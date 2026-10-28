use agent_core::{
    AppendRequest, Core, CoreRepository,
    turn_review::{ReviewKind, ReviewRequest, ReviewRun, ReviewStatus},
};
use agent_domain::{CommandId, ExecutorRef, ProjectId, RunId, TaskId};
use agent_protocol::{
    CommandEnvelope, CoreCommand, CreateTask, NewEvent, RegisterProject, StreamRef, StreamType,
    SubmitTurn,
};
use agent_store::SqliteStore;
use agent_workspace::GitWorkspaceContextProvider;
use driver_api::{
    AgentDriver, DriverError, DriverEvent, DriverEventStream, PromptRequest, SessionHandle,
    SessionRequest,
};
use futures::StreamExt;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Semaphore;

struct Reviewer {
    executor: ExecutorRef,
    mode: AtomicUsize,
    calls: Mutex<Vec<(String, PromptRequest)>>,
    stopped: Mutex<Vec<String>>,
    create_gate: Semaphore,
    creates: AtomicUsize,
    reject_creation: AtomicBool,
    hold_shutdown: AtomicBool,
    shutdown_gate: Semaphore,
    shutdowns: Mutex<Vec<String>>,
    close_error: AtomicBool,
    proof_enabled: AtomicBool,
    proof_gate: Arc<Semaphore>,
    proof_captures: AtomicUsize,
    handles: Arc<Mutex<std::collections::HashMap<String, SessionHandle>>>,
}
#[async_trait::async_trait]
impl AgentDriver for Reviewer {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "fixture"
    }
    fn supports_readonly_analysis(&self) -> bool {
        true
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        Ok(SessionHandle::new("main-session".into()))
    }
    async fn create_analysis_session(&self, _: &str) -> Result<SessionHandle, DriverError> {
        self.creates.fetch_add(1, Ordering::AcqRel);
        if self.mode.load(Ordering::SeqCst) == 9 {
            self.mode.store(10, Ordering::SeqCst); // Signal that startup has begun.
            self.create_gate.acquire().await.unwrap().forget();
        }
        if self.reject_creation.load(Ordering::Acquire) {
            return Err(DriverError::NotConfigured(
                "private native startup rejection".into(),
            ));
        }
        let handle = SessionHandle::scoped(format!("review-{}", RunId::new()));
        self.handles
            .lock()
            .unwrap()
            .insert(handle.native_session_id.clone(), handle.clone());
        Ok(handle)
    }
    async fn create_review_session(
        &self,
        directory: &str,
        _: &[String],
    ) -> Result<SessionHandle, DriverError> {
        self.create_analysis_session(directory).await
    }
    async fn respond_to_interaction(
        &self,
        handle: &SessionHandle,
        request: &str,
        option: &str,
    ) -> Result<(), DriverError> {
        assert!(handle.native_session_id.starts_with("review-"));
        assert_eq!(request, "read-git");
        assert_eq!(option, "vendor-allow-once");
        self.mode.store(13, Ordering::SeqCst);
        Ok(())
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        if handle.native_session_id == "main-session" {
            std::fs::write(
                std::path::Path::new(&request.working_directory).join("a.rs"),
                if self.mode.load(Ordering::SeqCst) == 11 {
                    "repaired version\n"
                } else {
                    "historical version\n"
                },
            )
            .unwrap();
        }
        self.calls
            .lock()
            .unwrap()
            .push((handle.native_session_id.clone(), request));
        if self.mode.load(Ordering::SeqCst) == 3 {
            return Ok(Box::pin(futures::stream::pending()));
        }
        if handle.native_session_id.starts_with("review-") && self.mode.load(Ordering::SeqCst) == 1
        {
            return Ok(Box::pin(futures::stream::pending()));
        }
        let mode = self.mode.load(Ordering::SeqCst);
        if mode == 17 {
            return Ok(Box::pin(
                futures::stream::iter([Ok(DriverEvent::TextDelta {
                    item_id: "partial".into(),
                    text: "已核对第一处调用方".into(),
                })])
                .chain(futures::stream::once(async {
                    panic!("private review stream panic")
                })),
            ));
        }
        if mode == 18 {
            self.handles
                .lock()
                .unwrap()
                .remove(&handle.native_session_id);
        }
        if mode == 16 {
            return Ok(Box::pin(futures::stream::iter(vec![
                Ok(DriverEvent::Presentation {
                    item_id: "progress".into(),
                    data: json!({"type":"message","text":"我正在检查分支"}),
                }),
                Ok(DriverEvent::Presentation {
                    item_id: "final".into(),
                    data: json!({"type":"message","text":r#"{"findings":[]}"#}),
                }),
                Ok(DriverEvent::TurnCompleted {
                    summary: "done".into(),
                }),
            ])));
        }
        if [14, 15].contains(&mode) {
            let events = vec![
                Ok(DriverEvent::ToolStarted {
                    item_id: "reading".into(),
                    tool_name: "Read".into(),
                    summary: "src/main.rs".into(),
                }),
                Ok(DriverEvent::Presentation {
                    item_id: "reply".into(),
                    data: json!({"type":"message", "textDelta":"正在核对异常路径"}),
                }),
            ];
            if mode == 14 {
                return Ok(Box::pin(
                    futures::stream::iter(events).chain(futures::stream::pending()),
                ));
            }
            return Ok(Box::pin(futures::stream::iter(events.into_iter().chain([
                Ok(DriverEvent::ToolCompleted {
                    item_id: "reading".into(),
                    tool_name: "Read".into(),
                    summary: "permission denied".into(),
                    success: false,
                }),
                Ok(DriverEvent::TurnFailed {
                    reason: "fixture disconnected".into(),
                }),
            ]))));
        }
        let mut events = vec![];
        if mode == 12 {
            use driver_api::{ApprovalOption, ApprovalSemanticRole};
            events.push(Ok(DriverEvent::ApprovalRequested {
                native_request_id: "read-git".into(),
                tool_name: "run_terminal_command".into(),
                command_summary: "git diff".into(),
                options: vec![
                    ApprovalOption {
                        option_id: "persistent".into(),
                        label: "always".into(),
                        semantic_role: ApprovalSemanticRole::ApprovePersistent,
                    },
                    ApprovalOption {
                        option_id: "vendor-allow-once".into(),
                        label: "once".into(),
                        semantic_role: ApprovalSemanticRole::ApproveOnce,
                    },
                ],
            }));
        }
        if mode == 2 {
            events.push(Ok(DriverEvent::ToolStarted {
                item_id: "read".into(),
                tool_name: "Read".into(),
                summary: "read current file".into(),
            }));
        }
        events.push(Ok(DriverEvent::TextDelta { item_id: "answer".into(), text: if mode == 4 {
            json!({"summary":"发现一个问题","findings":[{"root":"/project","path":"a.rs","severity":"high","title":"错误处理缺失","description":"失败路径会丢失错误","suggestion":"返回错误","side":"after","startLine":1,"endLine":1,"evidence":"code excerpt"}]}).to_string()
        } else if handle.native_session_id.starts_with("review-") {
            json!({"summary":"已检查当前代码，未发现问题","findings":[]}).to_string()
        } else { "已检查当前代码，建议覆盖异常路径".into() } }));
        events.push(Ok(DriverEvent::TurnCompleted {
            summary: "complete".into(),
        }));
        Ok(Box::pin(futures::stream::iter(events)))
    }
    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        self.stopped
            .lock()
            .unwrap()
            .push(handle.native_session_id.clone());
        Ok(())
    }
    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        if !handle.native_session_id.starts_with("review-")
            || !self.proof_enabled.load(Ordering::Acquire)
        {
            return None;
        }
        assert!(
            self.handles
                .lock()
                .unwrap()
                .get(&handle.native_session_id)
                .unwrap()
                .same_scope(handle)
        );
        self.proof_captures.fetch_add(1, Ordering::AcqRel);
        let proof = self.proof_gate.clone();
        let handles = self.handles.clone();
        let handle = handle.clone();
        Some(Box::pin(async move {
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
        self.shutdowns
            .lock()
            .unwrap()
            .push(handle.native_session_id.clone());
        if !handle.native_session_id.starts_with("review-") {
            return Ok(());
        }
        if self.hold_shutdown.load(Ordering::Acquire) {
            self.shutdown_gate.acquire().await.unwrap().forget();
        }
        if self.close_error.load(Ordering::Acquire) {
            return Err(DriverError::Disconnected("private close rejection".into()));
        }
        let mut handles = self.handles.lock().unwrap();
        if !handles
            .get(&handle.native_session_id)
            .is_some_and(|current| current.same_scope(&handle))
        {
            return Err(DriverError::SessionMissing(handle.native_session_id));
        }
        handles.remove(&handle.native_session_id);
        Ok(())
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<SqliteStore>,
    core: Arc<Core>,
    driver: Arc<Reviewer>,
    request: ReviewRequest,
}
fn review_probe(status: agent_protocol::InstallationStatus) -> CommandEnvelope<CoreCommand> {
    let mut command = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
        agent_protocol::RecordInstallationProbe {
            provider_id: "review-fixture".into(),
            executable: "test-review-agent".into(),
            fingerprint: "test-review-fingerprint".into(),
            status,
            version: None,
            version_text: None,
            diagnostic: None,
            elapsed_millis: 0,
        },
    ));
    command.actor = agent_protocol::ActorRef::SystemRecovery;
    command
}
async fn fixture() -> Fixture {
    fixture_with_capture(true).await
}
async fn fixture_with_capture(capture: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "before version\n").unwrap();
    std::fs::write(dir.path().join("helper.rs"), "pub fn guard() {}\n").unwrap();
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(if capture {
        Core::with_workspace_context(
            store.clone(),
            Arc::new(GitWorkspaceContextProvider::default()),
        )
    } else {
        Core::new(store.clone())
    });
    core.execute(review_probe(agent_protocol::InstallationStatus::Ready))
        .await
        .unwrap();
    let installation = core.bootstrap().await.unwrap().installations.remove(0);
    let executor = ExecutorRef {
        provider_id: agent_domain::ProviderId::parse("review-fixture").unwrap(),
        installation_id: Some(installation.installation_id.to_string()),
    };
    let driver = Arc::new(Reviewer {
        executor: executor.clone(),
        mode: AtomicUsize::new(0),
        calls: Mutex::default(),
        stopped: Mutex::default(),
        create_gate: Semaphore::new(0),
        creates: AtomicUsize::new(0),
        reject_creation: AtomicBool::new(false),
        hold_shutdown: AtomicBool::new(false),
        shutdown_gate: Semaphore::new(0),
        shutdowns: Mutex::default(),
        close_error: AtomicBool::new(false),
        proof_enabled: AtomicBool::new(false),
        proof_gate: Arc::new(Semaphore::new(0)),
        proof_captures: AtomicUsize::new(0),
        handles: Arc::default(),
    });
    core.register_driver(driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "review".into(),
            path: dir.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "source".into(),
            preferred_executor: executor.clone(),
        },
    )))
    .await
    .unwrap();
    submit(&core, &task, "实现历史版本").await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .is_none()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let turn = core
        .timeline(&task)
        .await
        .unwrap()
        .into_iter()
        .find_map(|i| i.turn_id)
        .unwrap();
    let request = ReviewRequest {
        recheck_of: None,
        task_id: task,
        turn_id: Some(turn),
        source_snapshot: None,
        run_id: RunId::new(),
        executor,
        kind: ReviewKind::Code,
        retry_of: None,
    };
    Fixture {
        dir,
        store,
        core,
        driver,
        request,
    }
}
async fn settled(f: &Fixture, id: &RunId) -> ReviewRun {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(run) = f
                .core
                .turn_reviews(&f.request.task_id)
                .await
                .unwrap()
                .into_iter()
                .find(|r| &r.request.run_id == id)
                && !matches!(
                    run.status,
                    ReviewStatus::Queued | ReviewStatus::Running | ReviewStatus::Cancelling
                )
            {
                return run;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn reviews_live_workspace_in_a_separate_session_and_retries_scope() {
    let f = fixture().await;
    let before = f.core.bootstrap().await.unwrap();
    std::fs::write(f.dir.path().join("a.rs"), "later live version").unwrap();
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let done = settled(&f, &f.request.run_id).await;
    assert_eq!(done.status, ReviewStatus::Completed);
    assert!(done.workspace_review);
    assert!(done.workflow.is_none());
    assert!(done.source_artifact.is_none());
    let (id, prompt) = f.driver.calls.lock().unwrap().last().unwrap().clone();
    assert_ne!(id, "main-session");
    assert!(!prompt.prompt.contains("historical version"));
    assert!(prompt.prompt.contains("a.rs"));
    assert!(!prompt.prompt.contains("later live version"));
    assert!(prompt.prompt.contains("实现历史版本"));
    assert_eq!(
        std::path::Path::new(&prompt.working_directory),
        f.dir.path()
    );
    assert!(prompt.additional_directories.is_empty());
    let after = f.core.bootstrap().await.unwrap();
    assert_eq!(
        before.tasks[0].preferred_executor,
        after.tasks[0].preferred_executor
    );
    assert_eq!(
        before.tasks[0].active_primary_run,
        after.tasks[0].active_primary_run
    );
    assert_eq!(before.session_maps, after.session_maps);
    let mut retry = f.request.clone();
    retry.run_id = RunId::new();
    retry.retry_of = Some(f.request.run_id.clone());
    f.core.start_turn_review(retry.clone()).await.unwrap();
    let repeated = settled(&f, &retry.run_id).await;
    assert_eq!(done.input_sha256, repeated.input_sha256);
    {
        let calls = f.driver.calls.lock().unwrap();
        assert_eq!(calls[1].1.prompt, calls[2].1.prompt);
        assert_ne!(calls[1].0, calls[2].0);
    }
    f.store.rebuild_projections().unwrap();
    let resumed = Core::new(f.store.clone());
    assert_eq!(
        resumed
            .turn_reviews(&f.request.task_id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("a.rs")).unwrap(),
        "later live version"
    );
}
#[tokio::test]
async fn completed_turn_without_snapshot_can_review_current_code() {
    let f = fixture_with_capture(false).await;
    let request = f.request.clone();
    f.core.start_turn_review(request.clone()).await.unwrap();
    let run = settled(&f, &request.run_id).await;
    assert_eq!(run.status, ReviewStatus::Completed);
    assert!(run.source_artifact.is_none());
    assert!(run.workspace_review);
}

#[tokio::test]
async fn review_answers_native_tool_permissions_without_affecting_main_session() {
    let f = fixture().await;
    let before = f.core.bootstrap().await.unwrap();
    f.driver.mode.store(12, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Completed
    );
    assert_eq!(f.driver.mode.load(Ordering::SeqCst), 13);
    assert_eq!(
        before.session_maps,
        f.core.bootstrap().await.unwrap().session_maps
    );
    let calls = f.driver.calls.lock().unwrap();
    assert!(
        calls
            .last()
            .unwrap()
            .1
            .prompt
            .contains("只能审查，不得修改任何代码")
    );
    assert!(calls.last().unwrap().1.prompt.contains("允许跨目录读取"));
}

#[tokio::test]
async fn review_can_be_cancelled_while_native_session_is_starting() {
    let f = fixture().await;
    let before = f.core.bootstrap().await.unwrap();
    f.driver.mode.store(9, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while f.driver.mode.load(Ordering::SeqCst) != 10 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.core
        .cancel_turn_review(&f.request.task_id, &f.request.run_id)
        .await
        .unwrap();
    // Accepted cancellation cannot abandon the original native creation.
    assert_eq!(current_review(&f).await.status, ReviewStatus::Cancelling);
    assert!(f.core.has_active_reviews().await);
    assert!(f.core.has_active_auxiliary_operations());
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert!(
        f.driver
            .shutdowns
            .lock()
            .unwrap()
            .iter()
            .all(|id| id == "main-session")
    );
    f.driver.create_gate.add_permits(1);
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Cancelled
    );
    assert_eq!(f.driver.calls.lock().unwrap().len(), 1); // Only the primary turn.
    assert_eq!(
        before.session_maps,
        f.core.bootstrap().await.unwrap().session_maps
    );
}

async fn current_review(f: &Fixture) -> ReviewRun {
    f.core
        .turn_reviews(&f.request.task_id)
        .await
        .unwrap()
        .into_iter()
        .find(|run| run.request.run_id == f.request.run_id)
        .unwrap()
}
async fn wait_review_boundary(test: impl Fn() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !test() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("private review boundary was not reached");
}
fn review_closes(driver: &Reviewer) -> usize {
    driver
        .shutdowns
        .lock()
        .unwrap()
        .iter()
        .filter(|id| id.starts_with("review-"))
        .count()
}

#[tokio::test]
async fn review_completion_waits_for_original_shutdown_and_cancel_still_wins() {
    let f = fixture().await;
    f.driver.hold_shutdown.store(true, Ordering::Release);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    wait_review_boundary(|| review_closes(&f.driver) == 1).await;
    assert_eq!(current_review(&f).await.status, ReviewStatus::Running);
    assert!(f.core.has_active_reviews().await);
    assert!(f.core.has_active_auxiliary_operations());
    f.core
        .cancel_turn_review(&f.request.task_id, &f.request.run_id)
        .await
        .unwrap();
    assert_eq!(current_review(&f).await.status, ReviewStatus::Cancelling);
    f.driver.shutdown_gate.add_permits(1);
    let review = settled(&f, &f.request.run_id).await;
    assert_eq!(review.status, ReviewStatus::Cancelled);
    assert!(review.result.contains("findings"));
    assert_eq!(review_closes(&f.driver), 1);
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn review_close_failure_stays_pending_until_its_original_proof_without_a_replay() {
    let f = fixture().await;
    f.driver.close_error.store(true, Ordering::Release);
    f.driver.proof_enabled.store(true, Ordering::Release);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let run = current_review(&f).await;
            if run
                .error
                .as_deref()
                .is_some_and(|message| message.contains("关闭尚未确认"))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pending = current_review(&f).await;
    assert_eq!(pending.status, ReviewStatus::Running);
    assert!(pending.result.contains("findings"));
    assert!(pending.finished_at.is_none());
    assert!(f.core.has_active_reviews().await);
    assert_eq!(review_closes(&f.driver), 1);
    assert_eq!(f.driver.proof_captures.load(Ordering::Acquire), 1);
    // Replacing the registry must not retarget the original close or proof.
    f.core
        .register_driver(Arc::new(driver_api::FakeDriver::with_executor(
            f.driver.executor.clone(),
        )))
        .await;
    f.driver.proof_gate.add_permits(1);
    let completed = settled(&f, &f.request.run_id).await;
    assert_eq!(completed.status, ReviewStatus::Completed);
    assert!(completed.error.is_none());
    assert_eq!(review_closes(&f.driver), 1);
    assert_eq!(f.driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(f.driver.proof_captures.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_panicking_review_stream_preserves_partial_output_and_retires_only_its_original_session()
{
    let f = fixture().await;
    let primary = f.core.bootstrap().await.unwrap().session_maps;
    f.driver.mode.store(17, Ordering::Release);
    f.driver.hold_shutdown.store(true, Ordering::Release);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    wait_review_boundary(|| review_closes(&f.driver) == 1).await;
    assert!(f.core.has_active_reviews().await);
    assert!(f.core.has_active_auxiliary_operations());
    f.driver.shutdown_gate.add_permits(1);
    let failed = settled(&f, &f.request.run_id).await;
    assert_eq!(failed.status, ReviewStatus::Failed);
    assert!(failed.result.contains("已核对第一处调用方"));
    assert!(failed.error.as_deref().unwrap().contains("意外中断"));
    assert_eq!(f.core.bootstrap().await.unwrap().session_maps, primary);
    assert_eq!(f.driver.stopped.lock().unwrap().len(), 1);
    assert!(f.driver.stopped.lock().unwrap()[0].starts_with("review-"));
    assert_eq!(review_closes(&f.driver), 1);
}

#[tokio::test]
async fn cancelled_startup_with_a_late_explicit_rejection_never_invents_a_native_shutdown() {
    let f = fixture().await;
    f.driver.mode.store(9, Ordering::Release);
    f.driver.reject_creation.store(true, Ordering::Release);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    wait_review_boundary(|| f.driver.mode.load(Ordering::Acquire) == 10).await;
    f.core
        .cancel_turn_review(&f.request.task_id, &f.request.run_id)
        .await
        .unwrap();
    assert_eq!(current_review(&f).await.status, ReviewStatus::Cancelling);
    assert!(f.core.has_active_reviews().await);
    f.driver.create_gate.add_permits(1);
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Cancelled
    );
    assert_eq!(review_closes(&f.driver), 0);
    assert!(f.driver.stopped.lock().unwrap().is_empty());
    assert_eq!(f.driver.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn review_retirement_uses_proof_captured_before_the_native_table_entry_disappears() {
    let f = fixture().await;
    f.driver.mode.store(18, Ordering::Release);
    f.driver.proof_enabled.store(true, Ordering::Release);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    wait_review_boundary(|| review_closes(&f.driver) == 1).await;
    assert!(f.driver.handles.lock().unwrap().is_empty());
    assert!(f.core.has_active_reviews().await);
    assert_eq!(f.driver.proof_captures.load(Ordering::Acquire), 1);
    assert_eq!(current_review(&f).await.status, ReviewStatus::Running);
    f.driver.proof_gate.add_permits(1);
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Completed
    );
    assert_eq!(review_closes(&f.driver), 1);
    assert_eq!(f.driver.proof_captures.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn duplicate_start_cancel_and_read_tools_do_not_affect_the_primary_session() {
    let f = fixture().await;
    f.driver.mode.store(1, Ordering::SeqCst);
    let (a, b) = tokio::join!(
        f.core.start_turn_review(f.request.clone()),
        f.core.start_turn_review(f.request.clone())
    );
    assert_eq!(a.unwrap().request.run_id, b.unwrap().request.run_id);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while f.driver.calls.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(f.core.has_active_reviews().await);
    f.core
        .cancel_turn_review(&f.request.task_id, &f.request.run_id)
        .await
        .unwrap();
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Cancelled
    );
    assert_eq!(f.driver.calls.lock().unwrap().len(), 2);
    assert!(
        f.driver
            .stopped
            .lock()
            .unwrap()
            .iter()
            .all(|id| id.starts_with("review-"))
    );
    f.driver.mode.store(2, Ordering::SeqCst);
    let mut next = f.request.clone();
    next.run_id = RunId::new();
    f.core.start_turn_review(next.clone()).await.unwrap();
    let result = settled(&f, &next.run_id).await;
    assert_eq!(result.status, ReviewStatus::Completed);
}
#[tokio::test]
async fn service_restart_marks_unowned_reviews_interrupted_without_replaying_them() {
    let f = fixture().await;
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let mut orphan = settled(&f, &f.request.run_id).await;
    orphan.request.run_id = RunId::new();
    orphan.status = ReviewStatus::Running;
    orphan.result.clear();
    orphan.activities.clear();
    orphan.activity_revision = 0;
    orphan.finished_at = None;
    f.store
        .append(AppendRequest {
            request_fingerprint: None,
            command_id: CommandId::new(),
            idempotency_key: CommandId::new().to_string(),
            expected_stream_version: None,
            artifacts: vec![],
            events: vec![NewEvent::hub(
                StreamRef {
                    stream_type: StreamType::Task,
                    stream_id: f.request.task_id.to_string(),
                },
                "review.updated",
                json!({"taskId":f.request.task_id,"turnId":f.request.turn_id,"review":orphan}),
            ), NewEvent::hub(
                StreamRef { stream_type: StreamType::Task, stream_id: f.request.task_id.to_string() },
                "review.activity", json!({"taskId":f.request.task_id,"runId":orphan.request.run_id,"revision":1,"activities":[{"itemId":"partial","data":{"type":"message","text":"重启前已有输出"}}]}),
            )],
        })
        .await
        .unwrap();
    let core = Core::new(f.store.clone());
    core.recover_interrupted_turns().await.unwrap();
    core.recover_interrupted_turns().await.unwrap();
    let runs = core.turn_reviews(&f.request.task_id).await.unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].status, ReviewStatus::Interrupted);
    assert_eq!(runs[1].activities[0].data["text"], "重启前已有输出");
    assert_eq!(f.driver.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn rejects_unfinished_foreign_turns_and_unsupported_executors() {
    let f = fixture().await;
    let mut request = f.request.clone();
    request.turn_id = Some(agent_domain::TurnId::new());
    assert!(f.core.start_turn_review(request).await.is_err());
    let mut request = f.request.clone();
    request.executor.provider_id = agent_domain::ProviderId::parse("unsupported").unwrap();
    assert!(f.core.start_turn_review(request).await.is_err());
    assert_eq!(f.driver.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn review_cancel_leaves_a_concurrent_primary_turn_running() {
    let f = fixture().await;
    f.driver.mode.store(3, Ordering::SeqCst);
    submit(&f.core, &f.request.task_id, "继续主任务").await;
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while f.driver.calls.lock().unwrap().len() < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let primary = f.core.bootstrap().await.unwrap().tasks[0]
        .active_primary_run
        .clone();
    assert!(primary.is_some());
    f.core
        .cancel_turn_review(&f.request.task_id, &f.request.run_id)
        .await
        .unwrap();
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Cancelled
    );
    assert_eq!(
        f.core.bootstrap().await.unwrap().tasks[0].active_primary_run,
        primary
    );
    assert!(
        f.driver
            .stopped
            .lock()
            .unwrap()
            .iter()
            .all(|id| id.starts_with("review-"))
    );
}

#[tokio::test]
async fn completed_review_returns_with_source_in_the_next_primary_prompt() {
    let f = fixture().await;
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let review = settled(&f, &f.request.run_id).await;
    submit(&f.core, &f.request.task_id, "继续处理").await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while f.driver.calls.lock().unwrap().len() < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let calls = f.driver.calls.lock().unwrap();
    let prompt = &calls.last().unwrap().1.prompt;
    assert!(prompt.contains("readonly_review"));
    assert!(prompt.contains(&review.input_sha256));
    assert!(prompt.contains(review.request.run_id.as_str()));
    assert_eq!(review.status, ReviewStatus::Completed);
    let sources = prompt
        .lines()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .expect("prompt contains structured review sources");
    let source = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["runId"] == review.request.run_id.as_str())
        .expect("completed review is included");
    assert_eq!(source["result"]["text"], review.result);
    assert_eq!(source["result"]["omittedBytes"], 0);
    assert!(prompt.ends_with("继续处理"));
}

async fn submit(core: &Arc<Core>, task: &TaskId, prompt: &str) {
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        executor_override: None,
        task_id: task.clone(),
        prompt: prompt.into(),
        model: None,
        thinking: None,
        permission_mode: None,
        attachments: vec![],
    }));
    command.expected_stream_version = Some(
        core.bootstrap()
            .await
            .unwrap()
            .tasks
            .iter()
            .find(|t| &t.task_id == task)
            .unwrap()
            .stream_version,
    );
    core.execute(command).await.unwrap();
}

/// Existing adapters need no readonly capability opt-in or analysis protocol.
struct ExistingAgent {
    executor: ExecutorRef,
    reviewer: Arc<Reviewer>,
    sessions: Arc<Mutex<Vec<SessionRequest>>>,
    independent: bool,
}
#[async_trait::async_trait]
impl AgentDriver for ExistingAgent {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "existing-driver"
    }
    fn independent_session_driver(&self) -> Result<Option<Arc<dyn AgentDriver>>, DriverError> {
        Ok(Some(Arc::new(Self {
            executor: self.executor.clone(),
            reviewer: self.reviewer.clone(),
            sessions: self.sessions.clone(),
            independent: true,
        })))
    }
    async fn permission_catalog(
        &self,
        _: Option<&str>,
    ) -> Result<driver_api::PermissionCatalog, DriverError> {
        Ok(driver_api::PermissionCatalog::new(
            "full-access",
            &[
                ("full-access", "bypass", "", true),
                ("plan", "plan", "", false),
            ],
        ))
    }
    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        assert!(
            self.independent,
            "must not reuse a connection carrying the primary turn"
        );
        let session = self
            .reviewer
            .create_analysis_session(&request.working_directory)
            .await?;
        self.sessions.lock().unwrap().push(request);
        Ok(session)
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        self.reviewer.prompt(handle, request).await
    }
    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        self.reviewer.interrupt(handle).await
    }
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        self.reviewer.shutdown(handle).await
    }
}

#[tokio::test]
async fn existing_agents_reuse_session_apis_without_a_readonly_capability_gate() {
    let f = fixture().await;
    let sessions = Arc::new(Mutex::new(vec![]));
    for provider in [
        "codex",
        "claude-code",
        "kimi",
        "cursor",
        "grok",
        "codebuddy",
        "zcode",
        "antigravity",
    ] {
        let mut probe = review_probe(agent_protocol::InstallationStatus::Ready);
        if let CoreCommand::RecordInstallationProbe(command) = &mut probe.payload {
            command.provider_id = provider.into();
        }
        f.core.execute(probe).await.unwrap();
        let installation = f
            .core
            .bootstrap()
            .await
            .unwrap()
            .installations
            .into_iter()
            .find(|installation| installation.provider_id == provider)
            .unwrap();
        let executor = ExecutorRef {
            provider_id: agent_domain::ProviderId::parse(provider).unwrap(),
            installation_id: Some(installation.installation_id.to_string()),
        };
        let driver = Arc::new(ExistingAgent {
            executor: executor.clone(),
            reviewer: f.driver.clone(),
            sessions: sessions.clone(),
            independent: false,
        });
        assert!(!driver.supports_readonly_analysis());
        f.core.register_driver(driver).await;
        assert!(f.core.review_executors().await.unwrap().contains(&executor));
        let request = ReviewRequest {
            recheck_of: None,
            executor,
            run_id: RunId::new(),
            ..f.request.clone()
        };
        f.core.start_turn_review(request.clone()).await.unwrap();
        assert_eq!(
            settled(&f, &request.run_id).await.status,
            ReviewStatus::Completed
        );
    }
    let sessions = sessions.lock().unwrap();
    assert_eq!(sessions.len(), 8);
    for request in sessions.iter() {
        assert_eq!(request.permission_mode.as_deref(), Some("full-access"));
        assert!(request.history_mcp.is_none());
        assert!(request.additional_directories.is_empty());
        assert_eq!(
            std::path::Path::new(&request.working_directory),
            f.dir.path()
        );
    }
}

#[tokio::test]
async fn retired_checks_remain_readable_but_cannot_start_or_retry() {
    let f = fixture().await;
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let original = settled(&f, &f.request.run_id).await;
    let calls = f.driver.calls.lock().unwrap().len();
    for kind in [
        ReviewKind::Tests,
        ReviewKind::Requirements,
        ReviewKind::Impact,
    ] {
        let mut historical = original.clone();
        historical.request.kind = kind;
        historical.request.run_id = RunId::new();
        f.store.append(AppendRequest {
            request_fingerprint: None,
            command_id: CommandId::new(),
            idempotency_key: CommandId::new().to_string(),
            expected_stream_version: None,
            artifacts: vec![],
            events: vec![NewEvent::hub(
                StreamRef { stream_type: StreamType::Task, stream_id: f.request.task_id.to_string() },
                "review.updated",
                json!({"taskId":f.request.task_id,"turnId":f.request.turn_id,"review":historical}),
            )],
        }).await.unwrap();
        for retry_of in [None, Some(historical.request.run_id.clone())] {
            let mut request = historical.request.clone();
            request.run_id = RunId::new();
            request.retry_of = retry_of;
            let error = f.core.start_turn_review(request).await.unwrap_err();
            assert!(error.to_string().contains("该检查类型已移除"));
        }
    }
    let runs = f.core.turn_reviews(&f.request.task_id).await.unwrap();
    assert_eq!(runs.len(), 4);
    assert!(runs.iter().all(|run| run.result == original.result));
    assert_eq!(f.driver.calls.lock().unwrap().len(), calls);
}

#[tokio::test]
async fn project_reviews_retry_scope_and_keep_turn_reviews_independent() {
    let f = fixture().await;
    let mut request = f.request.clone();
    request.turn_id = None;
    request.source_snapshot = Some("snapshot-project".into());
    let input = json!({"taskId":request.task_id,"snapshotId":"snapshot-project","scope":"已暂存 · base → target","roots":[{"path":f.dir.path(),"mode":"staged","files":[{"path":"a.rs","before":"old","after":"frozen project version"}]}]});
    let before = f.core.bootstrap().await.unwrap().tasks[0].clone();
    let run = f
        .core
        .start_project_review(request.clone(), Some(input.clone()))
        .await
        .unwrap();
    let done = settled(&f, &run.request.run_id).await;
    assert_eq!(done.status, ReviewStatus::Completed);
    assert!(done.coverage.contains("已暂存"));
    std::fs::write(f.dir.path().join("a.rs"), "later live change").unwrap();
    let replay = f
        .core
        .start_project_review(request.clone(), None)
        .await
        .unwrap();
    assert_eq!(replay.request.run_id, request.run_id);
    let mut retry = request.clone();
    retry.run_id = RunId::new();
    retry.retry_of = Some(request.run_id.clone());
    let retried = f
        .core
        .start_project_review(retry.clone(), None)
        .await
        .unwrap();
    let retried = settled(&f, &retried.request.run_id).await;
    assert_eq!(done.input_sha256, retried.input_sha256);
    {
        let calls = f.driver.calls.lock().unwrap();
        assert_eq!(calls[1].1.prompt, calls[2].1.prompt);
        assert!(!calls[1].1.prompt.contains("frozen project version"));
        assert!(!calls[1].1.prompt.contains("later live change"));
    }
    let after = f.core.bootstrap().await.unwrap().tasks[0].clone();
    assert_eq!(before.preferred_executor, after.preferred_executor);
    assert_eq!(before.active_primary_run, after.active_primary_run);
    let mut wrong_source = retry.clone();
    wrong_source.run_id = RunId::new();
    wrong_source.source_snapshot = Some("different-snapshot".into());
    assert!(
        f.core
            .start_project_review(wrong_source, None)
            .await
            .is_err()
    );
    assert!(f.core.start_turn_review(request.clone()).await.is_err());
    assert!(
        f.core
            .start_project_review(f.request.clone(), Some(input))
            .await
            .is_err()
    );
    f.store.rebuild_projections().unwrap();
    let restored = f.core.turn_reviews(&request.task_id).await.unwrap();
    assert_eq!(restored.len(), 2);
    assert!(restored.iter().all(|run| run.request.turn_id.is_none()));
    let timeline = f.core.timeline(&request.task_id).await.unwrap();
    assert!(
        timeline
            .iter()
            .any(|item| item.kind == "review_result" && item.turn_id.is_none())
    );
    // Both kinds share the cancellation machinery, but each has its own run.
    f.driver.mode.store(1, Ordering::SeqCst);
    let mut turn = f.request.clone();
    turn.run_id = RunId::new();
    f.core.start_turn_review(turn.clone()).await.unwrap();
    retry.run_id = RunId::new();
    f.core
        .start_project_review(retry.clone(), None)
        .await
        .unwrap();
    f.core
        .cancel_turn_review(&retry.task_id, &retry.run_id)
        .await
        .unwrap();
    assert_eq!(
        settled(&f, &retry.run_id).await.status,
        ReviewStatus::Cancelled
    );
    let runs = f.core.turn_reviews(&request.task_id).await.unwrap();
    assert!(runs.iter().any(|run| run.request.run_id == turn.run_id
        && matches!(run.status, ReviewStatus::Queued | ReviewStatus::Running)));
    f.core
        .cancel_turn_review(&turn.task_id, &turn.run_id)
        .await
        .unwrap();
    assert_eq!(
        settled(&f, &turn.run_id).await.status,
        ReviewStatus::Cancelled
    );
}

#[tokio::test]
async fn structured_findings_survive_rebuild_without_frozen_sources() {
    let f = fixture().await;
    f.driver.mode.store(4, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let done = settled(&f, &f.request.run_id).await;
    assert_eq!(done.status, ReviewStatus::Completed);
    assert_eq!(done.report.as_ref().unwrap().findings.len(), 1);
    assert!(!done.report.as_ref().unwrap().findings[0].location_verified);
    assert!(done.workflow.is_none());
    f.store.rebuild_projections().unwrap();
    let restored = Core::new(f.store.clone())
        .turn_reviews(&f.request.task_id)
        .await
        .unwrap();
    assert_eq!(
        restored[0].report.as_ref().unwrap().findings[0]
            .finding
            .title,
        "错误处理缺失"
    );
    assert!(
        f.core
            .review_source(&f.request.task_id, &f.request.run_id, "issue-1")
            .await
            .is_err()
    );
    assert_eq!(f.driver.calls.lock().unwrap().len(), 2); // One main turn, one review; no extra AI pass.
}

#[tokio::test]
async fn historical_review_records_without_new_fields_remain_readable() {
    let f = fixture().await;
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let done = settled(&f, &f.request.run_id).await;
    assert_eq!(done.status, ReviewStatus::Completed);
    assert!(done.report.is_some());
    let mut value = serde_json::to_value(&done).unwrap();
    for key in ["report", "workspaceReview", "workflow"] {
        value.as_object_mut().unwrap().remove(key);
    }
    let legacy: ReviewRun = serde_json::from_value(value).unwrap();
    assert!(!legacy.workspace_review);
    assert!(legacy.report.is_none());
    assert_eq!(legacy.result, done.result);
}

#[tokio::test]
async fn selected_findings_repair_and_recheck_current_code_survive_rebuild() {
    use agent_core::turn_review::ReviewRecheck;
    use agent_protocol::ReviewFixRef;
    let f = fixture().await;
    f.driver.mode.store(4, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let original = settled(&f, &f.request.run_id).await;
    assert_eq!(original.report.as_ref().unwrap().findings.len(), 1);
    let reference = ReviewFixRef {
        run_id: original.request.run_id.clone(),
        input_sha256: original.input_sha256.clone(),
        finding_indices: vec![0],
    };
    let command = |reference: ReviewFixRef| {
        CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            review_fix: Some(reference),
            task_id: f.request.task_id.clone(),
            prompt: "修复选中项".into(),
            attachments: vec![],
            permission_mode: None,
            model: None,
            thinking: None,
            executor_override: None,
        }))
    };
    for invalid in [
        ReviewFixRef {
            input_sha256: "stale".into(),
            ..reference.clone()
        },
        ReviewFixRef {
            finding_indices: vec![99],
            ..reference.clone()
        },
        ReviewFixRef {
            finding_indices: vec![0, 0],
            ..reference.clone()
        },
        ReviewFixRef {
            run_id: RunId::new(),
            ..reference.clone()
        },
    ] {
        let mut cmd = command(invalid);
        cmd.expected_stream_version =
            Some(f.core.bootstrap().await.unwrap().tasks[0].stream_version);
        assert!(f.core.execute(cmd).await.is_err());
    }
    f.driver.mode.store(11, Ordering::SeqCst);
    let mut cmd = command(reference);
    cmd.expected_stream_version = Some(f.core.bootstrap().await.unwrap().tasks[0].stream_version);
    let ack = f.core.execute(cmd.clone()).await.unwrap();
    let again = f.core.execute(cmd).await.unwrap();
    assert_eq!(ack, again);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let runs = f.core.turn_reviews(&f.request.task_id).await.unwrap();
    let parent = runs
        .iter()
        .find(|r| r.request.run_id == original.request.run_id)
        .unwrap();
    assert_eq!(parent.repairs.len(), 1);
    assert_eq!(parent.repairs[0].status, "completed");
    let fix_prompt = f
        .driver
        .calls
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(id, _)| id == "main-session")
        .unwrap()
        .1
        .prompt
        .clone();
    assert!(fix_prompt.contains("错误处理缺失"));
    assert!(fix_prompt.contains("返回错误"));
    let repair_turn = parent.repairs[0].turn_id.clone();
    let mut request = ReviewRequest {
        run_id: RunId::new(),
        turn_id: Some(repair_turn.clone()),
        recheck_of: Some(ReviewRecheck {
            run_id: original.request.run_id.clone(),
            repair_turn_id: repair_turn,
        }),
        ..f.request.clone()
    };
    let mut wrong = request.clone();
    wrong.recheck_of.as_mut().unwrap().run_id = RunId::new();
    assert!(f.core.start_turn_review(wrong).await.is_err());
    std::fs::write(f.dir.path().join("a.rs"), "later unrelated live version").unwrap();
    f.core.start_turn_review(request.clone()).await.unwrap();
    let rechecked = settled(&f, &request.run_id).await;
    assert_ne!(rechecked.input_sha256, original.input_sha256);
    let prompt = f
        .driver
        .calls
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .1
        .prompt
        .clone();
    assert!(!prompt.contains("repaired version"));
    assert!(prompt.contains("当前实现"));
    assert!(prompt.contains("repairReview"));
    assert!(!prompt.contains("later unrelated live version"));
    request.retry_of = Some(request.run_id.clone());
    request.run_id = RunId::new();
    f.core.start_turn_review(request.clone()).await.unwrap();
    assert_eq!(
        settled(&f, &request.run_id).await.input_sha256,
        rechecked.input_sha256
    );
    f.store.rebuild_projections().unwrap();
    let runs = f.core.turn_reviews(&f.request.task_id).await.unwrap();
    let parent = runs
        .iter()
        .find(|r| r.request.run_id == original.request.run_id)
        .unwrap();
    assert_eq!(parent.input_sha256, original.input_sha256);
    assert_eq!(parent.repairs[0].finding_indices, vec![0]);
    assert_eq!(parent.repairs[0].recheck_runs.len(), 2);
}

#[tokio::test]
async fn deleted_review_is_permanently_removed_without_touching_workspace() {
    use agent_protocol::TaskLifecycle;
    let f = fixture().await;
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    settled(&f, &f.request.run_id).await;
    let file = f.dir.path().join("a.rs");
    std::fs::write(&file, "new live version").unwrap();
    let snapshot = f.core.bootstrap().await.unwrap();
    let mut delete = CommandEnvelope::local(CoreCommand::DeleteTask(TaskLifecycle {
        task_id: f.request.task_id.clone(),
    }));
    delete.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    f.core.execute(delete).await.unwrap();
    assert!(
        f.core
            .review_source(&f.request.task_id, &f.request.run_id, "file-1")
            .await
            .is_err()
    );
    let mut retry = f.request.clone();
    retry.run_id = RunId::new();
    retry.retry_of = Some(f.request.run_id.clone());
    assert!(f.core.start_turn_review(retry.clone()).await.is_err());
    f.store.rebuild_projections().unwrap();
    let restarted = Arc::new(Core::new(f.store.clone()));
    let snapshot = restarted.bootstrap().await.unwrap();
    assert!(snapshot.tasks.is_empty());
    assert!(snapshot.deleted_tasks.is_empty());
    assert!(
        restarted
            .review_source(&f.request.task_id, &f.request.run_id, "file-1")
            .await
            .is_err()
    );
    assert!(restarted.turn_reviews(&f.request.task_id).await.is_err());
    assert!(f.core.start_turn_review(retry).await.is_err());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "new live version");
}

#[tokio::test]
async fn review_stream_is_durable_before_completion_and_cancel_preserves_partial_output() {
    let f = fixture().await;
    let main_before = f.core.timeline(&f.request.task_id).await.unwrap();
    f.driver.mode.store(14, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let progress = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let run = f
                .core
                .turn_reviews(&f.request.task_id)
                .await
                .unwrap()
                .pop()
                .unwrap();
            if run.activity_revision > 0 {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(progress.status, ReviewStatus::Running);
    assert_eq!(progress.activities.len(), 2);
    assert_eq!(progress.activities[0].data["status"], "running");
    assert_eq!(progress.activities[1].data["text"], "正在核对异常路径");
    assert!(progress.report.is_none());
    assert_eq!(
        f.core.timeline(&f.request.task_id).await.unwrap().len(),
        main_before.len()
    );
    f.core
        .cancel_turn_review(&f.request.task_id, &f.request.run_id)
        .await
        .unwrap();
    let run = settled(&f, &f.request.run_id).await;
    assert_eq!(run.status, ReviewStatus::Cancelled);
    assert_eq!(run.result, "正在核对异常路径");
    assert_eq!(run.activities.len(), 2);
    assert!(run.report.is_none());
    f.store.rebuild_projections().unwrap();
    let restarted = Core::new(f.store.clone());
    let saved = restarted
        .turn_reviews(&f.request.task_id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(saved.status, ReviewStatus::Cancelled);
    assert_eq!(saved.result, run.result);
    assert_eq!(saved.activities[1].data, run.activities[1].data);
}

#[tokio::test]
async fn review_failure_flushes_even_the_tail_before_the_first_stream_tick() {
    let f = fixture().await;
    f.driver.mode.store(15, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let run = settled(&f, &f.request.run_id).await;
    assert_eq!(run.status, ReviewStatus::Failed);
    assert_eq!(run.result, "正在核对异常路径");
    assert_eq!(run.activities.len(), 2);
    assert_eq!(run.activities[0].data["status"], "failed");
    assert_eq!(run.activities[0].data["input"], "src/main.rs");
    assert_eq!(run.activities[0].data["output"], "permission denied");
    assert!(run.report.is_none());
}

#[tokio::test]
async fn final_native_message_becomes_report_while_progress_is_retained() {
    let f = fixture().await;
    f.driver.mode.store(16, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let run = settled(&f, &f.request.run_id).await;
    assert_eq!(run.status, ReviewStatus::Completed);
    assert!(run.report.as_ref().unwrap().structured);
    assert_eq!(run.activities.len(), 2);
    assert_eq!(run.activities[0].data["text"], "我正在检查分支");
}

async fn set_review_disabled(core: &Arc<Core>, disabled: bool) {
    let current = core.agent_preferences().await.unwrap();
    let mut preferences = current.preferences;
    preferences.disabled = if disabled {
        vec!["review-fixture".into()]
    } else {
        vec![]
    };
    let mut command = CommandEnvelope::local(CoreCommand::SetAgentPreferences(preferences));
    command.expected_stream_version = Some(current.revision);
    core.execute(command).await.unwrap();
}

#[tokio::test]
async fn review_admission_blocks_disabled_agents_but_preserves_accepted_work_and_replay() {
    let f = fixture().await;
    f.driver.mode.store(1, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    set_review_disabled(&f.core, true).await;
    assert!(f.core.review_executors().await.unwrap().is_empty());
    let replay = f.core.start_turn_review(f.request.clone()).await.unwrap();
    assert!(matches!(
        replay.status,
        ReviewStatus::Queued | ReviewStatus::Running
    ));
    let mut fresh = f.request.clone();
    fresh.run_id = RunId::new();
    assert!(
        f.core
            .start_turn_review(fresh)
            .await
            .unwrap_err()
            .to_string()
            .contains("已禁用")
    );
    assert_eq!(
        f.core.turn_reviews(&f.request.task_id).await.unwrap().len(),
        1
    );
    assert!(f.core.has_active_reviews().await);
    f.core
        .cancel_turn_review(&f.request.task_id, &f.request.run_id)
        .await
        .unwrap();
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Cancelled
    );
    set_review_disabled(&f.core, false).await;
    assert_eq!(
        f.core.review_executors().await.unwrap(),
        std::slice::from_ref(&f.request.executor)
    );
}

#[tokio::test]
async fn review_admission_rejects_nonready_missing_and_unselected_installations() {
    use agent_protocol::InstallationStatus;
    let f = fixture().await;
    for status in [
        InstallationStatus::Installed,
        InstallationStatus::NotConfigured,
        InstallationStatus::UnsupportedVersion,
        InstallationStatus::Broken,
    ] {
        f.core.execute(review_probe(status)).await.unwrap();
        assert!(f.core.review_executors().await.unwrap().is_empty());
        assert!(
            f.core
                .start_turn_review(f.request.clone())
                .await
                .unwrap_err()
                .to_string()
                .contains("尚未就绪")
        );
    }
    f.core
        .execute(review_probe(InstallationStatus::Ready))
        .await
        .unwrap();
    for id in [None, Some(agent_domain::InstallationId::new().to_string())] {
        let mut request = f.request.clone();
        request.executor.installation_id = id;
        assert!(
            f.core
                .start_turn_review(request)
                .await
                .unwrap_err()
                .to_string()
                .contains("尚未就绪")
        );
    }
    f.core
        .select_installation("review-fixture", Some("different-executable".into()))
        .await;
    f.core.register_driver(f.driver.clone()).await; // Late registration from the old probe.
    assert!(f.core.review_executors().await.unwrap().is_empty());
    assert!(
        f.core
            .start_turn_review(f.request.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("尚未就绪")
    );
    assert!(
        f.core
            .turn_reviews(&f.request.task_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.driver.calls.lock().unwrap().len(), 1); // Only fixture's main turn.
    f.core
        .select_installation("review-fixture", Some("test-review-agent".into()))
        .await;
    f.core.register_driver(f.driver.clone()).await;
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Completed
    );
}

#[tokio::test]
async fn review_admission_replays_and_cancels_after_installation_removal() {
    let f = fixture().await;
    f.driver.mode.store(1, Ordering::SeqCst);
    f.core.start_turn_review(f.request.clone()).await.unwrap();
    let installation = f.core.bootstrap().await.unwrap().installations.remove(0);
    let mut removed = CommandEnvelope::local(CoreCommand::ReconcileMissingInstallation(
        agent_protocol::ReconcileMissingInstallation {
            installation_id: installation.installation_id,
            provider_id: installation.provider_id,
            executable: installation.executable,
        },
    ));
    removed.actor = agent_protocol::ActorRef::SystemRecovery;
    f.core.execute(removed).await.unwrap();
    assert!(f.core.review_executors().await.unwrap().is_empty());
    assert!(matches!(
        f.core
            .start_turn_review(f.request.clone())
            .await
            .unwrap()
            .status,
        ReviewStatus::Queued | ReviewStatus::Running
    ));
    let mut fresh = f.request.clone();
    fresh.run_id = RunId::new();
    assert!(
        f.core
            .start_turn_review(fresh)
            .await
            .unwrap_err()
            .to_string()
            .contains("尚未就绪")
    );
    f.core
        .cancel_turn_review(&f.request.task_id, &f.request.run_id)
        .await
        .unwrap();
    assert_eq!(
        settled(&f, &f.request.run_id).await.status,
        ReviewStatus::Cancelled
    );
}
