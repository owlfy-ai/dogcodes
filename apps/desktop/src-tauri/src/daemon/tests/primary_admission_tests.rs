use super::*;
use agent_protocol::{AgentPreferences, CreateTask, SubmitTurn};
use driver_api::AgentDriver;

async fn settings(f: &Fixture, disabled: Vec<String>) {
    let current = f.core.agent_preferences().await.unwrap();
    let mut request = CommandEnvelope::local(CoreCommand::SetAgentPreferences(AgentPreferences {
        disabled,
        ..Default::default()
    }));
    request.expected_stream_version = Some(current.revision);
    tokio::time::timeout(Duration::from_secs(3), f.client.execute(request))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn primary_admission_rpc_rejects_disabled_work_and_replays_accepted_work() {
    let f = fixture().await;
    let driver = Arc::new(FakeDriver::with_executor(ready(&f, "fake").await));
    let other_driver = Arc::new(FakeDriver::with_executor(ready(&f, "other").await));
    f.core.register_driver(driver.clone()).await;
    f.core.register_driver(other_driver.clone()).await;
    let project_id = ProjectId::new();
    let task_id = TaskId::new();
    f.client
        .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project_id.clone(),
                display_name: "admission".into(),
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
                title: "admission".into(),
                preferred_executor: driver.executor().clone(),
            },
        )))
        .await
        .unwrap();
    let mut request = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task_id.clone(),
        prompt: "original work".into(),
        executor_override: None,
        attachments: vec![],
        model: None,
        thinking: None,
        permission_mode: None,
        review_fix: None,
    }));
    request.expected_stream_version = Some(1);
    let other = CoreClient::Remote(f.directory.path().into());
    settings(&f, vec!["fake".into()]).await;
    assert!(
        other
            .execute(request.clone())
            .await
            .unwrap_err()
            .contains("已禁用")
    );
    assert!(
        f.repository
            .command_ack(&request.idempotency_key)
            .await
            .unwrap()
            .is_none()
    );
    assert!(driver.received_requests().await.is_empty());

    settings(&f, vec![]).await;
    // Hold only execution preparation, proving the settings gate is released
    // once accepted even if this run has not reached the driver yet.
    let preparation = f.core.try_workspace_mutation().unwrap();
    let accepted = other.execute(request.clone()).await.unwrap();
    settings(&f, vec!["fake".into()]).await;
    f.core.select_installation("fake", None).await;
    let replacement = Arc::new(FakeDriver::with_executor(driver.executor().clone()));
    f.core.register_driver(replacement.clone()).await;
    assert_eq!(other.execute(request.clone()).await.unwrap(), accepted);
    assert!(driver.received_requests().await.is_empty());
    drop(preparation);
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.core.bootstrap().await.unwrap().tasks[0]
            .active_primary_run
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(driver.received_requests().await.len(), 1);
    assert!(replacement.received_requests().await.is_empty());
    assert_eq!(other.execute(request.clone()).await.unwrap(), accepted);
    let mut changed = request.clone();
    if let CoreCommand::SubmitTurn(command) = &mut changed.payload {
        command.prompt = "different work".into();
    }
    assert!(other.execute(changed).await.is_err());
    assert_eq!(driver.received_requests().await.len(), 1);
    assert!(replacement.received_requests().await.is_empty());

    // Validate the actual override, rather than only the task's preferred Agent.
    settings(&f, vec!["other".into()]).await;
    let CoreCommand::SubmitTurn(mut command) = request.payload else {
        unreachable!()
    };
    command.executor_override = Some(other_driver.executor().clone());
    let mut overridden = CommandEnvelope::local(CoreCommand::SubmitTurn(command));
    overridden.expected_stream_version =
        Some(f.core.bootstrap().await.unwrap().tasks[0].stream_version);
    assert!(
        other
            .execute(overridden.clone())
            .await
            .unwrap_err()
            .contains("已禁用")
    );
    assert!(
        f.repository
            .command_ack(&overridden.idempotency_key)
            .await
            .unwrap()
            .is_none()
    );
    assert!(other_driver.received_requests().await.is_empty());
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
async fn primary_admission_rpc_requires_ready_explicit_current_installation() {
    use agent_protocol::{ActorRef, InstallationStatus, RecordInstallationProbe};
    let f = fixture().await;
    let executor = ready(&f, "fake").await;
    let driver = Arc::new(FakeDriver::with_executor(executor.clone()));
    f.core.register_driver(driver.clone()).await;
    let project_id = ProjectId::new();
    let task_id = TaskId::new();
    f.client
        .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project_id.clone(),
                display_name: "readiness".into(),
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
                title: "readiness".into(),
                preferred_executor: executor.clone(),
            },
        )))
        .await
        .unwrap();
    let request = |executor_override| {
        let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            task_id: task_id.clone(),
            prompt: "work".into(),
            executor_override,
            attachments: vec![],
            model: None,
            thinking: None,
            permission_mode: None,
            review_fix: None,
        }));
        command.expected_stream_version = Some(1);
        command
    };
    let other = CoreClient::Remote(f.directory.path().into());
    for status in [
        InstallationStatus::Installed,
        InstallationStatus::NotConfigured,
        InstallationStatus::Broken,
        InstallationStatus::UnsupportedVersion,
        InstallationStatus::Ready,
    ] {
        let mut probe = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            RecordInstallationProbe {
                provider_id: "fake".into(),
                executable: "fixture:fake".into(),
                fingerprint: "fixture".into(),
                status,
                version: None,
                version_text: None,
                diagnostic: None,
                elapsed_millis: 0,
            },
        ));
        probe.actor = ActorRef::SystemRecovery;
        f.core.execute(probe).await.unwrap();
        if status == InstallationStatus::Ready {
            break;
        }
        let command = request(None);
        assert!(
            other
                .execute(command.clone())
                .await
                .unwrap_err()
                .contains("尚未就绪或已更换")
        );
        assert!(
            f.repository
                .command_ack(&command.idempotency_key)
                .await
                .unwrap()
                .is_none()
        );
    }
    for identity in [None, Some(agent_domain::InstallationId::new().to_string())] {
        let command = request(Some(agent_domain::ExecutorRef {
            installation_id: identity,
            ..executor.clone()
        }));
        assert!(
            other
                .execute(command.clone())
                .await
                .unwrap_err()
                .contains("尚未就绪或已更换")
        );
        assert!(
            f.repository
                .command_ack(&command.idempotency_key)
                .await
                .unwrap()
                .is_none()
        );
    }
    let command = request(Some(agent_domain::ExecutorRef {
        provider_id: agent_domain::ProviderId::parse("other").unwrap(),
        ..executor.clone()
    }));
    assert!(
        other
            .execute(command)
            .await
            .unwrap_err()
            .contains("尚未就绪或已更换")
    );
    // A late re-registration cannot resurrect a removed or deselected installation.
    for selected in [None, Some("fixture:replacement".into())] {
        f.core.select_installation("fake", selected).await;
        f.core.register_driver(driver.clone()).await;
        let command = request(None);
        assert!(
            other
                .execute(command.clone())
                .await
                .unwrap_err()
                .contains("尚未就绪或已更换")
        );
        assert!(
            f.repository
                .command_ack(&command.idempotency_key)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(driver.received_requests().await.is_empty());
    assert!(
        f.core
            .sync_events(0, 1000)
            .await
            .unwrap()
            .iter()
            .all(|event| event.event_type != "turn.queued")
    );
}
