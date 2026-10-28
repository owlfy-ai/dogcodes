use super::*;
use crate::tests::MemoryRepository;
use driver_api::{AgentDriver, FakeDriver};

#[tokio::test]
async fn abandoning_an_original_worker_wakes_cancellation_without_claiming_native_completion() {
    let core = Core::new(Arc::new(MemoryRepository::default()));
    let driver: Arc<dyn AgentDriver> = Arc::new(FakeDriver::new("fake").unwrap());
    let run = RunId::new();
    let ownership = core.own_run(run.clone(), driver);
    let control = ownership.control.clone();
    assert!(control.begin_dispatch());
    control.request_stop();
    let mut waiting = Box::pin(control.stop_target());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiting)
            .await
            .is_err()
    );
    drop(ownership);
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .unwrap(),
        crate::run_control::StopTarget::Abandoned
    ));
    assert!(!control.finished());
    assert!(!core.owned_controls.lock().unwrap().contains_key(&run));
}

struct ScopedDriver {
    inner: FakeDriver,
    handle: driver_api::SessionHandle,
    cancels: std::sync::atomic::AtomicUsize,
    approvals: std::sync::atomic::AtomicUsize,
    questions: std::sync::atomic::AtomicUsize,
}
impl ScopedDriver {
    fn check(&self, handle: &driver_api::SessionHandle) -> Result<(), driver_api::DriverError> {
        if self.handle.same_scope(handle) {
            Ok(())
        } else {
            Err(driver_api::DriverError::SessionIncompatible(
                "wrong original scope".into(),
            ))
        }
    }
}
#[async_trait::async_trait]
impl AgentDriver for ScopedDriver {
    fn executor(&self) -> &agent_domain::ExecutorRef {
        self.inner.executor()
    }
    fn driver_version(&self) -> &'static str {
        "private-scope-fixture"
    }
    async fn create_session(
        &self,
        _: driver_api::SessionRequest,
    ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
        Ok(self.handle.clone())
    }
    async fn resume_session(
        &self,
        _: &str,
        _: &str,
    ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
        Ok(self.handle.clone())
    }
    async fn prompt(
        &self,
        handle: &driver_api::SessionHandle,
        request: driver_api::PromptRequest,
    ) -> Result<driver_api::DriverEventStream, driver_api::DriverError> {
        self.check(handle)?;
        self.inner.prompt(handle, request).await
    }
    async fn interrupt(
        &self,
        handle: &driver_api::SessionHandle,
    ) -> Result<(), driver_api::DriverError> {
        self.check(handle)?;
        self.cancels
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(())
    }
    async fn respond_to_interaction(
        &self,
        handle: &driver_api::SessionHandle,
        _: &str,
        _: &str,
    ) -> Result<(), driver_api::DriverError> {
        self.check(handle)?;
        self.approvals
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(())
    }
    async fn respond_to_question(
        &self,
        handle: &driver_api::SessionHandle,
        _: &str,
        _: agent_domain::QuestionAnswer,
    ) -> Result<(), driver_api::DriverError> {
        self.check(handle)?;
        self.questions
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(())
    }
    async fn shutdown(
        &self,
        handle: driver_api::SessionHandle,
    ) -> Result<(), driver_api::DriverError> {
        self.check(&handle)
    }
}

#[tokio::test]
async fn cancellation_and_approval_use_original_runtime_handles_not_database_reconstruction() {
    use agent_domain::{CommandId, ProjectId, TaskId, TurnId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let repository = Arc::new(MemoryRepository::default());
    let core = Core::new(repository.clone());
    let concrete = Arc::new(ScopedDriver {
        inner: FakeDriver::new("fake").unwrap(),
        handle: driver_api::SessionHandle::scoped("original-native-session".into()),
        cancels: AtomicUsize::new(0),
        approvals: AtomicUsize::new(0),
        questions: AtomicUsize::new(0),
    });
    let driver: Arc<dyn AgentDriver> = concrete.clone();
    let executor = driver.executor().clone();
    let run = RunId::new();
    let task = TaskId::new();
    let turn = TurnId::new();
    *repository.task_override.lock().await = Some(agent_protocol::TaskSummary {
        last_turn_executor: None,
        updated_at: None,
        task_id: task.clone(),
        project_id: ProjectId::new(),
        title: "scope fixture".into(),
        preferred_executor: executor.clone(),
        stream_version: 1,
        active_primary_run: Some(agent_protocol::ActiveRunSummary {
            run_id: run.clone(),
            turn_id: turn.clone(),
            executor: executor.clone(),
            state: RunState::Running,
            started_at: chrono::Utc::now(),
        }),
    });
    // No session map exists: reconstructing from the projection or run ID
    // cannot yield authority over this native session.
    let ownership = core.own_run(run.clone(), driver.clone());
    assert!(core.run_session(&run, &executor).is_err());
    core.bind_run_session(&run, &driver, concrete.handle.clone())
        .unwrap();
    assert!(
        core.bind_run_session(
            &run,
            &driver,
            driver_api::SessionHandle::scoped("replacement".into())
        )
        .is_err()
    );
    let replacement: Arc<dyn AgentDriver> = Arc::new(FakeDriver::new("fake").unwrap());
    core.register_driver(replacement).await;
    let control = ownership.control.clone();
    control.ready();
    core.cancel_turn_inner(
        &agent_protocol::CancelTurn {
            task_id: task.clone(),
            turn_id: turn.clone(),
            run_id: run.clone(),
        },
        Some(&control),
    )
    .await
    .unwrap();
    for question in [false, true] {
        core.deliver_approval_response(
            agent_protocol::RespondToApproval {
                task_id: task.clone(),
                turn_id: turn.clone(),
                requested_at_sequence: 0,
                approval_id: "current-request".into(),
                option_id: "allow".into(),
                answer: question.then_some(agent_domain::QuestionAnswer {
                    cancelled: true,
                    values: serde_json::Value::Null,
                }),
            },
            Some(turn.clone()),
            CommandId::new(),
        )
        .await;
        assert_eq!(
            core.repository
                .task_events(&task, 0, u64::MAX, 100)
                .await
                .unwrap()
                .last()
                .unwrap()
                .event_type,
            "approval.delivery_sent"
        );
    }
    assert_eq!(concrete.cancels.load(Ordering::Acquire), 1);
    assert_eq!(concrete.approvals.load(Ordering::Acquire), 1);
    assert_eq!(concrete.questions.load(Ordering::Acquire), 1);
    control.settling();
    assert!(
        !control.finished(),
        "local result still needs to be persisted"
    );
    core.cancel_turn_inner(
        &agent_protocol::CancelTurn {
            task_id: task.clone(),
            turn_id: turn.clone(),
            run_id: run.clone(),
        },
        Some(&control),
    )
    .await
    .unwrap();
    core.deliver_approval_response(
        agent_protocol::RespondToApproval {
            task_id: task.clone(),
            turn_id: turn.clone(),
            requested_at_sequence: 0,
            approval_id: "current-request".into(),
            option_id: "allow".into(),
            answer: None,
        },
        Some(turn.clone()),
        CommandId::new(),
    )
    .await;
    assert_eq!(
        concrete.cancels.load(Ordering::Acquire),
        1,
        "late stop reached settled native work"
    );
    assert_eq!(
        concrete.approvals.load(Ordering::Acquire),
        1,
        "late approval reached settled native work"
    );
    assert_eq!(
        core.repository
            .task_events(&task, 0, u64::MAX, 100)
            .await
            .unwrap()
            .last()
            .unwrap()
            .event_type,
        "approval.delivery_failed"
    );
    drop(ownership);
    assert!(core.run_session(&run, &executor).is_err());
    assert!(!core.owned_sessions.lock().unwrap().contains_key(&run));
    assert!(
        core.cancel_turn_inner(
            &agent_protocol::CancelTurn {
                task_id: task,
                turn_id: turn,
                run_id: run
            },
            Some(&control)
        )
        .await
        .is_err()
    );
    assert_eq!(concrete.cancels.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn run_driver_survives_selection_and_same_identity_replacement_then_releases() {
    let core = Core::new(Arc::new(MemoryRepository::default()));
    let original: Arc<dyn AgentDriver> = Arc::new(FakeDriver::new("fake").unwrap());
    let weak = Arc::downgrade(&original);
    let executor = original.executor().clone();
    core.register_driver(original.clone()).await;
    let run = RunId::new();
    let ownership = core.own_run(run.clone(), original.clone());
    core.select_installation("fake", None).await;
    let replacement: Arc<dyn AgentDriver> = Arc::new(FakeDriver::new("fake").unwrap());
    core.register_driver(replacement.clone()).await;
    assert!(Arc::ptr_eq(
        &original,
        &core.run_driver(&run, &executor).unwrap()
    ));
    assert!(!Arc::ptr_eq(
        &replacement,
        &core.run_driver(&run, &executor).unwrap()
    ));
    let wrong = agent_domain::ExecutorRef {
        installation_id: Some("other".into()),
        ..executor.clone()
    };
    assert!(core.run_driver(&run, &wrong).is_err());
    assert!(core.run_driver(&RunId::new(), &executor).is_err());
    drop(original);
    assert!(weak.upgrade().is_some());
    drop(ownership);
    assert!(weak.upgrade().is_none());
    assert!(core.run_driver(&run, &executor).is_err());
    assert_eq!(core.registered_executors().await, vec![executor]);
}
