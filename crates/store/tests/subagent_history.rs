//! Durable child-history routing; only in-memory drivers and private `SQLite`.
#[path = "support/installations.rs"]
mod installations;
use agent_core::Core;
use agent_domain::{ExecutorRef, InstallationId, ProjectId, TaskId};
use agent_protocol::*;
use agent_store::SqliteStore;
use driver_api::{
    AgentDriver, DriverError, DriverEvent, DriverEventStream, PromptRequest, SessionHandle,
    SessionRequest,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::mpsc;

struct HistoryDriver {
    executor: ExecutorRef,
    label: &'static str,
    reads: AtomicUsize,
    stream: Mutex<Option<mpsc::UnboundedSender<Result<DriverEvent, DriverError>>>>,
    automatic: Mutex<Option<mpsc::UnboundedReceiver<Result<DriverEvent, DriverError>>>>,
}
impl HistoryDriver {
    fn new(executor: ExecutorRef, label: &'static str) -> Arc<Self> {
        Arc::new(Self {
            executor,
            label,
            reads: AtomicUsize::new(0),
            stream: Mutex::new(None),
            automatic: Mutex::new(None),
        })
    }
}
#[async_trait::async_trait]
impl AgentDriver for HistoryDriver {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "private-history-fixture"
    }
    fn supports_live_session_reuse(&self) -> bool {
        true
    }
    async fn take_autonomous_events(&self, _: &SessionHandle) -> Option<DriverEventStream> {
        let receive = self.automatic.lock().unwrap().take()?;
        Some(Box::pin(futures::stream::unfold(
            receive,
            |mut receive| async { receive.recv().await.map(|event| (event, receive)) },
        )))
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        Ok(SessionHandle::scoped(format!("native-{}", self.label)))
    }
    async fn resume_session(&self, _: &str, _: &str) -> Result<SessionHandle, DriverError> {
        unreachable!()
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        _: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        let (send, receive) = mpsc::unbounded_channel();
        send.send(Ok(DriverEvent::Presentation {
            item_id: "child-card".into(),
            data: json!({
                "type":"subagent","childId":"same-child","details":"history","status":"running"
            }),
        }))
        .unwrap();
        *self.stream.lock().unwrap() = Some(send);
        Ok(Box::pin(futures::stream::unfold(
            receive,
            |mut receive| async { receive.recv().await.map(|event| (event, receive)) },
        )))
    }
    async fn read_subagent(&self, id: &str) -> Result<Vec<Value>, DriverError> {
        assert_eq!(id, "same-child");
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(vec![json!({"type":"message","text":self.label})])
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}
async fn wait_for(core: &Core, task: &TaskId, finished: bool, marker: &str) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let timeline = core.timeline(task).await.unwrap();
            let active = core
                .bootstrap()
                .await
                .unwrap()
                .tasks
                .iter()
                .find(|t| &t.task_id == task)
                .unwrap()
                .active_primary_run
                .is_some();
            if active != finished
                && let Some(item) = timeline
                    .iter()
                    .find(|item| item.kind == "activity" && item.text.contains(marker))
            {
                return item.item_id.clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn child_history_uses_its_durable_installation_and_original_running_driver() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let original = HistoryDriver::new(installations::executor("fake"), "original");
    let (automatic, receive) = mpsc::unbounded_channel();
    *original.automatic.lock().unwrap() = Some(receive);
    let other = HistoryDriver::new(
        ExecutorRef {
            installation_id: Some(InstallationId::new().to_string()),
            ..original.executor.clone()
        },
        "other-installation",
    );
    installations::register(&core, store.as_ref(), original.clone()).await;
    installations::register(&core, store.as_ref(), other.clone()).await;
    let root = tempfile::tempdir().unwrap();
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "private-history".into(),
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
            title: "private task".into(),
            preferred_executor: original.executor.clone(),
        },
    )))
    .await
    .unwrap();
    let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: "private fixture".into(),
        attachments: vec![],
        permission_mode: None,
        model: None,
        thinking: None,
        executor_override: None,
        review_fix: None,
    }));
    submit.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(submit).await.unwrap();
    let item = wait_for(&core, &task, false, "same-child").await;
    assert_eq!(
        core.subagent_history(&task, &item).await.unwrap()[0]["text"],
        "original"
    );
    let replacement =
        HistoryDriver::new(original.executor.clone(), "replacement-same-installation");
    core.register_driver(replacement.clone()).await;
    assert_eq!(
        core.subagent_history(&task, &item).await.unwrap()[0]["text"],
        "original"
    );
    assert_eq!(replacement.reads.load(Ordering::SeqCst), 0);
    original
        .stream
        .lock()
        .unwrap()
        .take()
        .unwrap()
        .send(Ok(DriverEvent::TurnCompleted {
            summary: "done".into(),
        }))
        .unwrap();
    wait_for(&core, &task, true, "same-child").await;
    let mut select =
        CommandEnvelope::local(CoreCommand::SetPreferredExecutor(SetPreferredExecutor {
            task_id: task.clone(),
            executor: other.executor.clone(),
        }));
    select.expected_stream_version = Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    core.execute(select).await.unwrap();
    assert_eq!(
        core.subagent_history(&task, &item).await.unwrap()[0]["text"],
        "replacement-same-installation"
    );
    assert_eq!(other.reads.load(Ordering::SeqCst), 0);
    automatic
        .send(Ok(DriverEvent::TurnStarted {
            native_turn_id: Some("auto-history".into()),
        }))
        .unwrap();
    automatic.send(Ok(DriverEvent::Presentation {item_id:"auto-card".into(),data:json!({
        "type":"subagent","childId":"same-child","details":"history_partial","summary":"auto-history","status":"running"
    })})).unwrap();
    let auto_item = wait_for(&core, &task, false, "auto-history").await;
    assert_eq!(
        core.subagent_history(&task, &auto_item).await.unwrap()[0]["text"],
        "original"
    );
    automatic
        .send(Ok(DriverEvent::TurnCompleted {
            summary: "done".into(),
        }))
        .unwrap();
    wait_for(&core, &task, true, "auto-history").await;
    assert_eq!(
        core.subagent_history(&task, &auto_item).await.unwrap()[0]["text"],
        "replacement-same-installation"
    );
    store.rebuild_projections().unwrap();
    let reopened = Core::new(store);
    reopened.register_driver(other.clone()).await;
    assert!(
        reopened.subagent_history(&task, &item).await.is_err(),
        "a sole different installation is never a substitute"
    );
    assert_eq!(other.reads.load(Ordering::SeqCst), 0);
    assert!(reopened.subagent_history(&task, &auto_item).await.is_err());
    reopened.register_driver(replacement).await;
    assert_eq!(
        reopened.subagent_history(&task, &item).await.unwrap()[0]["text"],
        "replacement-same-installation"
    );
    assert_eq!(
        reopened.subagent_history(&task, &auto_item).await.unwrap()[0]["text"],
        "replacement-same-installation"
    );
    assert_eq!(other.reads.load(Ordering::SeqCst), 0);
    assert_eq!(original.reads.load(Ordering::SeqCst), 3);
}
