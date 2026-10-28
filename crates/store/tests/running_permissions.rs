#[path = "support/installations.rs"]
mod installations;
use agent_core::Core;
use agent_domain::{ExecutorRef, ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use async_trait::async_trait;
use driver_api::*;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::{Semaphore, mpsc, oneshot};

struct ApprovalDriver {
    executor: ExecutorRef,
    question: bool,
    pause: Option<Arc<Semaphore>>,
    decision: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    replies: Mutex<Vec<String>>,
    interrupts: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl AgentDriver for ApprovalDriver {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    fn resume_capability(&self) -> SessionResumeCapability {
        SessionResumeCapability::Unsupported
    }
    async fn permission_catalog(&self, _: Option<&str>) -> Result<PermissionCatalog, DriverError> {
        Ok(PermissionCatalog::new(
            "default",
            &[
                ("default", "Confirm", "", false),
                ("full-access", "Full", "", true),
            ],
        ))
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        Ok(SessionHandle::new("session".into()))
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        _: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        let (tx, rx) = mpsc::unbounded_channel();
        let decision = Arc::clone(&self.decision);
        let question = self.question;
        let pause = self.pause.clone();
        tokio::spawn(async move {
            for index in 0..2 {
                let (reply, receive) = oneshot::channel();
                *decision.lock().unwrap() = Some(reply);
                let event = if question {
                    DriverEvent::QuestionRequested {
                        native_request_id: format!("request-{index}"),
                        form: agent_domain::QuestionForm {
                            title: "Choose".into(),
                            schema: json!({"type":"object"}),
                        },
                    }
                } else {
                    DriverEvent::ApprovalRequested {
                        native_request_id: format!("request-{index}"),
                        tool_name: "shell".into(),
                        command_summary: "example".into(),
                        options: vec![
                            ApprovalOption {
                                option_id: "vendor-allow".into(),
                                label: "Allow".into(),
                                semantic_role: ApprovalSemanticRole::ApproveOnce,
                            },
                            ApprovalOption {
                                option_id: "vendor-deny".into(),
                                label: "Deny".into(),
                                semantic_role: ApprovalSemanticRole::Reject,
                            },
                        ],
                    }
                };
                if tx.send(Ok(event)).is_err() || receive.await.is_err() {
                    return;
                }
                if index == 0
                    && let Some(pause) = &pause
                {
                    pause.acquire().await.unwrap().forget();
                }
            }
            let _ = tx.send(Ok(DriverEvent::TurnCompleted {
                summary: "done".into(),
            }));
        });
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })))
    }
    async fn respond_to_interaction(
        &self,
        _: &SessionHandle,
        _: &str,
        option: &str,
    ) -> Result<(), DriverError> {
        self.replies.lock().unwrap().push(option.into());
        self.decision
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        Ok(())
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        self.interrupts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}

async fn setup(
    question: bool,
    pause: Option<Arc<Semaphore>>,
) -> (Arc<Core>, Arc<ApprovalDriver>, TaskId) {
    let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
    let driver = Arc::new(ApprovalDriver {
        executor: installations::probe(&core, "cursor").await,
        question,
        pause,
        decision: Arc::default(),
        replies: Mutex::default(),
        interrupts: std::sync::atomic::AtomicUsize::default(),
    });
    core.register_driver(driver.clone()).await;
    let project_id = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project_id.clone(),
            display_name: "Test".into(),
            path: "/tmp".into(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    let task_id = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task_id.clone(),
            project_id,
            title: "Live permissions".into(),
            preferred_executor: driver.executor.clone(),
        },
    )))
    .await
    .unwrap();
    (core, driver, task_id)
}

async fn start(core: &Arc<Core>, task: &TaskId, mode: Option<&str>) {
    let snapshot = core.bootstrap().await.unwrap();
    let mut request = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: "test".into(),
        model: None,
        thinking: None,
        permission_mode: mode.map(Into::into),
        attachments: vec![],
        executor_override: None,
        review_fix: None,
    }));
    request.expected_stream_version = Some(
        snapshot
            .tasks
            .iter()
            .find(|t| &t.task_id == task)
            .unwrap()
            .stream_version,
    );
    core.execute(request).await.unwrap();
}

async fn pending(core: &Core, task: &TaskId) -> ActiveRunSummary {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if !core.pending_approvals(task, 50).await.unwrap().is_empty() {
                return core
                    .bootstrap()
                    .await
                    .unwrap()
                    .tasks
                    .into_iter()
                    .find(|t| &t.task_id == task)
                    .unwrap()
                    .active_primary_run
                    .unwrap();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

async fn idle(core: &Core) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while !core.bootstrap().await.unwrap().active_runs.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn full_access_handles_existing_and_future_requests_without_leaking_into_the_next_turn() {
    let (core, driver, task) = setup(false, None).await;
    start(&core, &task, None).await;
    let run = pending(&core, &task).await;
    let wrong = ExecutorRef {
        installation_id: Some("ins_other".into()),
        ..driver.executor.clone()
    };
    assert!(
        core.set_running_permission_mode(&task, &run.run_id, &wrong, "full-access")
            .await
            .is_err()
    );
    assert!(driver.replies.lock().unwrap().is_empty());
    core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
        .await
        .unwrap();
    idle(&core).await;
    assert_eq!(
        *driver.replies.lock().unwrap(),
        ["vendor-allow", "vendor-allow"]
    );
    assert!(core.pending_approvals(&task, 50).await.unwrap().is_empty());
    let events = core.sync_events(0, 1000).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "approval.resolved"
                && e.payload["permissionMode"] == "full-access")
            .count(),
        2
    );
    start(&core, &task, None).await;
    let next = pending(&core, &task).await;
    assert_ne!(run.run_id, next.run_id);
    assert!(
        core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
            .await
            .is_err()
    );
    assert_eq!(driver.replies.lock().unwrap().len(), 2);
    core.set_running_permission_mode(&task, &next.run_id, &driver.executor, "full-access")
        .await
        .unwrap();
    idle(&core).await;
}

#[tokio::test]
async fn reverting_live_full_access_restores_confirmations_and_reports_the_actual_policy() {
    let pause = Arc::new(Semaphore::new(0));
    let (core, driver, task) = setup(false, Some(pause.clone())).await;
    start(&core, &task, None).await;
    let run = pending(&core, &task).await;
    core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
        .await
        .unwrap();
    assert_eq!(
        core.permission_catalog(&driver.executor, Some(&task))
            .await
            .unwrap()
            .default_mode_id
            .as_deref(),
        Some("full-access")
    );
    core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "default")
        .await
        .unwrap();
    pause.add_permits(1);
    pending(&core, &task).await;
    assert_eq!(driver.replies.lock().unwrap().len(), 1);
    assert_eq!(
        core.permission_catalog(&driver.executor, Some(&task))
            .await
            .unwrap()
            .default_mode_id
            .as_deref(),
        Some("default")
    );
    core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
        .await
        .unwrap();
    idle(&core).await;
}

#[tokio::test]
async fn full_access_never_answers_questions_or_claims_to_restrict_a_force_started_process() {
    let (core, driver, task) = setup(true, None).await;
    start(&core, &task, Some("full-access")).await;
    let run = pending(&core, &task).await;
    core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
        .await
        .unwrap();
    assert!(driver.replies.lock().unwrap().is_empty());
    assert_eq!(core.pending_approvals(&task, 50).await.unwrap().len(), 1);
    assert!(
        core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "default")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn concurrent_manual_answer_and_full_access_deliver_each_request_once() {
    let (core, driver, task) = setup(false, None).await;
    start(&core, &task, None).await;
    let run = pending(&core, &task).await;
    let version = core.bootstrap().await.unwrap().tasks[0].stream_version;
    let approval = core.pending_approvals(&task, 50).await.unwrap().remove(0);
    let mut answer = CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
        turn_id: approval.turn_id,
        requested_at_sequence: approval.requested_at_sequence,
        task_id: task.clone(),
        approval_id: "request-0".into(),
        option_id: "vendor-allow".into(),
        answer: None,
    }));
    answer.expected_stream_version = Some(version);
    let (manual, full) = tokio::join!(
        core.execute(answer),
        core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
    );
    assert!(manual.is_ok() || full.is_ok());
    full.unwrap();
    idle(&core).await;
    assert_eq!(
        *driver.replies.lock().unwrap(),
        ["vendor-allow", "vendor-allow"]
    );
    assert_eq!(
        core.sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.event_type == "approval.resolved")
            .count(),
        2
    );
}

#[tokio::test]
async fn stopping_a_run_rejects_a_late_full_access_change() {
    let (core, driver, task) = setup(false, None).await;
    start(&core, &task, None).await;
    let run = pending(&core, &task).await;
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: task.clone(),
        turn_id: run.turn_id.clone(),
        run_id: run.run_id.clone(),
    }));
    cancel.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(cancel).await.unwrap();
    assert!(
        core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
            .await
            .is_err()
    );
    assert!(driver.replies.lock().unwrap().is_empty());
}

async fn disable_running_agent(core: &Arc<Core>) {
    let mut disable = CommandEnvelope::local(CoreCommand::SetAgentPreferences(AgentPreferences {
        disabled: vec!["cursor".into()],
        ..Default::default()
    }));
    disable.expected_stream_version = Some(0);
    tokio::time::timeout(std::time::Duration::from_secs(3), core.execute(disable))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn disabled_agent_keeps_accepted_turn_approvals_and_permission_updates() {
    let (core, driver, task) = setup(false, None).await;
    start(&core, &task, None).await;
    let run = pending(&core, &task).await;
    disable_running_agent(&core).await;
    let approval = core.pending_approvals(&task, 50).await.unwrap().remove(0);
    let mut answer = CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
        task_id: task.clone(),
        turn_id: approval.turn_id,
        requested_at_sequence: approval.requested_at_sequence,
        approval_id: "request-0".into(),
        option_id: "vendor-allow".into(),
        answer: None,
    }));
    answer.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(answer).await.unwrap();
    core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
        .await
        .unwrap();
    idle(&core).await;
    assert_eq!(
        *driver.replies.lock().unwrap(),
        ["vendor-allow", "vendor-allow"]
    );
}

#[tokio::test]
async fn disabled_agent_keeps_accepted_turn_cancellation() {
    let (core, _, task) = setup(false, None).await;
    start(&core, &task, None).await;
    let run = pending(&core, &task).await;
    disable_running_agent(&core).await;
    let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
        task_id: task,
        turn_id: run.turn_id,
        run_id: run.run_id,
    }));
    cancel.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    let ack = core.execute(cancel.clone()).await.unwrap();
    assert_eq!(core.execute(cancel).await.unwrap(), ack);
}

// A refresh can replace the adapter even when its public executor is unchanged.
async fn replace_running_driver(
    core: &Core,
    original: &ApprovalDriver,
    same_identity: bool,
) -> Arc<ApprovalDriver> {
    core.select_installation("cursor", Some("replacement-cursor".into()))
        .await;
    let mut executor = original.executor.clone();
    if !same_identity {
        executor.installation_id = Some("ins_replacement".into());
    }
    let replacement = Arc::new(ApprovalDriver {
        executor,
        question: false,
        pause: None,
        decision: Arc::default(),
        replies: Mutex::default(),
        interrupts: std::sync::atomic::AtomicUsize::default(),
    });
    core.register_driver(replacement.clone()).await;
    replacement
}

#[tokio::test]
async fn installation_switch_keeps_manual_and_automatic_approval_on_original_driver() {
    for same_identity in [false, true] {
        let (core, driver, task) = setup(false, None).await;
        start(&core, &task, None).await;
        let run = pending(&core, &task).await;
        let replacement = replace_running_driver(&core, &driver, same_identity).await;
        let approval = core.pending_approvals(&task, 50).await.unwrap().remove(0);
        let mut answer =
            CommandEnvelope::local(CoreCommand::RespondToApproval(RespondToApproval {
                task_id: task.clone(),
                turn_id: approval.turn_id,
                requested_at_sequence: approval.requested_at_sequence,
                approval_id: "request-0".into(),
                option_id: "vendor-allow".into(),
                answer: None,
            }));
        answer.expected_stream_version =
            Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
        core.execute(answer).await.unwrap();
        // Wait for manual delivery before switching the automatic policy.
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while driver.replies.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            core.permission_catalog(&driver.executor, Some(&task))
                .await
                .is_ok()
        );
        core.set_running_permission_mode(&task, &run.run_id, &driver.executor, "full-access")
            .await
            .unwrap();
        idle(&core).await;
        assert_eq!(
            *driver.replies.lock().unwrap(),
            ["vendor-allow", "vendor-allow"]
        );
        assert!(replacement.replies.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn installation_switch_cancels_only_original_driver() {
    for same_identity in [false, true] {
        let (core, driver, task) = setup(false, None).await;
        start(&core, &task, None).await;
        let run = pending(&core, &task).await;
        let replacement = replace_running_driver(&core, &driver, same_identity).await;
        let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
            task_id: task,
            turn_id: run.turn_id,
            run_id: run.run_id,
        }));
        cancel.expected_stream_version =
            Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
        let ack = core.execute(cancel.clone()).await.unwrap();
        assert_eq!(core.execute(cancel).await.unwrap(), ack);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while driver.interrupts.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            driver.interrupts.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            replacement
                .interrupts
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
}
