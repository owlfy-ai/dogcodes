#[path = "support/installations.rs"]
mod installations;
use agent_core::{Core, CoreError};
use agent_domain::{AttachmentIntegrity, ExecutorRef, ProjectId, PromptAttachment, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn};
use agent_store::SqliteStore;
use async_trait::async_trait;
use driver_api::{
    AgentDriver, DriverError, DriverEventStream, FakeDriver, PromptRequest, SessionHandle,
    SessionRequest,
};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

struct SessionWriter {
    inner: FakeDriver,
    change: Option<PathBuf>,
}
#[async_trait]
impl AgentDriver for SessionWriter {
    fn executor(&self) -> &ExecutorRef {
        self.inner.executor()
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        if let Some(path) = &self.change {
            std::fs::write(path, b"modified").unwrap();
        }
        self.inner.create_session(request).await
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        self.inner.prompt(handle, request).await
    }
    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        self.inner.interrupt(handle).await
    }
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        self.inner.shutdown(handle).await
    }
}
async fn fixture(
    change_during_session: bool,
) -> (
    Arc<Core>,
    Arc<SessionWriter>,
    CommandEnvelope<CoreCommand>,
    tempfile::TempDir,
) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("file.txt");
    std::fs::write(&path, b"original").unwrap();
    let core = Arc::new(Core::new(Arc::new(SqliteStore::open_in_memory().unwrap())));
    let driver = Arc::new(SessionWriter {
        inner: FakeDriver::with_executor(installations::probe(&core, "fake").await),
        change: change_during_session.then(|| path.clone()),
    });
    core.register_driver(driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "test".into(),
            path: directory.path().to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "test".into(),
            preferred_executor: driver.executor().clone(),
        },
    )))
    .await
    .unwrap();
    let mut envelope = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task,
        prompt: String::new(),
        review_fix: None,
        model: None,
        thinking: None,
        permission_mode: None,
        executor_override: None,
        attachments: vec![PromptAttachment {
            name: "file.txt".into(),
            path: path.to_string_lossy().into_owned(),
            mime_type: "text/plain".into(),
            integrity: Some(AttachmentIntegrity {
                sha256: format!("{:x}", Sha256::digest(b"original")),
                byte_length: 8,
            }),
        }],
    }));
    envelope.expected_stream_version =
        Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
    (core, driver, envelope, directory)
}
async fn idle(core: &Core) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn attachment_integrity_is_bound_to_the_command_and_history() {
    let (core, driver, envelope, _directory) = fixture(false).await;
    let original = envelope.clone();
    let ack = core.execute(envelope).await.unwrap();
    idle(&core).await;
    assert_eq!(driver.inner.received_requests().await.len(), 1);
    let CoreCommand::SubmitTurn(command) = &original.payload else {
        panic!()
    };
    let timeline = core.timeline(&command.task_id).await.unwrap();
    assert_eq!(
        timeline
            .iter()
            .find(|item| item.kind == "user_message")
            .unwrap()
            .attachments,
        command.attachments
    );
    std::fs::write(&command.attachments[0].path, b"modified").unwrap();
    assert_eq!(core.command_receipt(&original).await.unwrap(), Some(ack));
    let mut changed = original.clone();
    if let CoreCommand::SubmitTurn(command) = &mut changed.payload {
        command.attachments[0].integrity.as_mut().unwrap().sha256 = "0".repeat(64);
    }
    assert!(core.command_receipt(&changed).await.is_err());
}
#[tokio::test]
async fn attachment_changed_before_admission_is_not_queued() {
    let (core, driver, envelope, _directory) = fixture(false).await;
    let CoreCommand::SubmitTurn(command) = &envelope.payload else {
        panic!()
    };
    std::fs::write(&command.attachments[0].path, b"modified").unwrap();
    assert!(matches!(
        core.execute(envelope).await,
        Err(CoreError::InvalidArgument(_))
    ));
    assert!(driver.inner.received_requests().await.is_empty());
    assert!(
        !core
            .sync_events(0, 100)
            .await
            .unwrap()
            .iter()
            .any(|event| event.event_type == "turn.queued")
    );
}
#[tokio::test]
async fn attachment_changed_after_admission_fails_before_prompt_dispatch() {
    let (core, driver, envelope, _directory) = fixture(true).await;
    core.execute(envelope).await.unwrap();
    idle(&core).await;
    assert!(driver.inner.received_requests().await.is_empty());
    let events = core.sync_events(0, 100).await.unwrap();
    assert!(events.iter().any(|event| event.event_type == "turn.queued"));
    assert!(events.iter().any(|event| event.event_type == "turn.failed"));
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == "turn.completed")
    );
}
#[test]
fn legacy_attachment_serialization_does_not_change_existing_command_fingerprints() {
    let old = serde_json::json!({ "name": "file.txt", "path": "/tmp/file.txt", "mimeType": "text/plain" });
    let reference: PromptAttachment = serde_json::from_value(old.clone()).unwrap();
    assert!(reference.integrity.is_none());
    assert_eq!(serde_json::to_value(reference).unwrap(), old);
}
