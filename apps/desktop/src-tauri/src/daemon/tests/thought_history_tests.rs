use super::*;
use agent_protocol::{CreateTask, SubmitTurn};
use driver_api::{AgentDriver, DriverEvent};

/// Streams two thinking deltas before its answer.
struct Thinking {
    fake: FakeDriver,
}
#[async_trait::async_trait]
impl AgentDriver for Thinking {
    fn executor(&self) -> &ExecutorRef {
        self.fake.executor()
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    async fn create_session(
        &self,
        request: driver_api::SessionRequest,
    ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
        self.fake.create_session(request).await
    }
    async fn prompt(
        &self,
        _: &driver_api::SessionHandle,
        _: driver_api::PromptRequest,
    ) -> Result<driver_api::DriverEventStream, driver_api::DriverError> {
        let thought = |delta: &str| {
            Ok(DriverEvent::Presentation {
                item_id: "native-thought".into(),
                data: json!({"type": "thought", "delta": delta}),
            })
        };
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(DriverEvent::TurnStarted {
                native_turn_id: Some("native-turn".into()),
            }),
            thought("先分析问题"),
            thought("，再修改代码。"),
            Ok(DriverEvent::TextDelta {
                item_id: "answer".into(),
                text: "已完成".into(),
            }),
            Ok(DriverEvent::TurnCompleted {
                summary: "done".into(),
            }),
        ])))
    }
    async fn interrupt(
        &self,
        _: &driver_api::SessionHandle,
    ) -> Result<(), driver_api::DriverError> {
        Ok(())
    }
    async fn shutdown(&self, _: driver_api::SessionHandle) -> Result<(), driver_api::DriverError> {
        Ok(())
    }
}

async fn ready(f: &Fixture, provider: &str) -> agent_domain::ExecutorRef {
    let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
        agent_protocol::RecordInstallationProbe {
            provider_id: provider.into(),
            executable: format!("fixture:{provider}"),
            fingerprint: "fixture".into(),
            status: agent_protocol::InstallationStatus::Ready,
            version: None,
            version_text: None,
            diagnostic: None,
            elapsed_millis: 0,
        },
    ));
    probe.actor = agent_protocol::ActorRef::SystemRecovery;
    f.core.execute(probe).await.unwrap();
    let installation = f
        .core
        .bootstrap()
        .await
        .unwrap()
        .installations
        .into_iter()
        .find(|i| i.provider_id == provider)
        .unwrap();
    agent_domain::ExecutorRef {
        provider_id: agent_domain::ProviderId::parse(provider).unwrap(),
        installation_id: Some(installation.installation_id.to_string()),
    }
}

#[tokio::test]
async fn finished_thinking_is_saved_with_its_text_for_history() {
    let f = fixture().await;
    let driver = Arc::new(Thinking {
        fake: FakeDriver::with_executor(ready(&f, "fake").await),
    });
    f.core.register_driver(driver.clone()).await;
    let (project_id, task_id) = (ProjectId::new(), TaskId::new());
    f.client
        .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project_id.clone(),
                display_name: "thinking".into(),
                path: f.directory.path().to_string_lossy().into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
    f.client
        .execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task_id.clone(),
                project_id,
                title: "thinking".into(),
                preferred_executor: driver.executor().clone(),
            },
        )))
        .await
        .unwrap();
    let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task_id.clone(),
        prompt: "修一下".into(),
        executor_override: None,
        attachments: vec![],
        model: None,
        thinking: None,
        permission_mode: None,
        review_fix: None,
    }));
    submit.expected_stream_version = Some(1);
    f.client.execute(submit).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let snapshot = f.core.timeline_snapshot(&task_id).await.unwrap();
    let thought: Value = snapshot
        .items
        .iter()
        .filter(|item| item.kind == "activity")
        .map(|item| serde_json::from_str::<Value>(&item.text).unwrap())
        .find(|data| data["type"] == "thought")
        .expect("a thinking row");
    assert_eq!(thought["status"], "completed");
    assert_eq!(thought["text"], "先分析问题，再修改代码。");
    assert!(thought.get("textTruncated").is_none());
    assert!(thought.get("liveText").is_none());
}
