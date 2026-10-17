//! A second client that reaches the service only through its RPC protocol, the
//! way the planned mobile client will (through a future gateway). No window,
//! Tauri state or client-side storage is involved.
use super::*;
use crate::project_registry::ProjectRegistration;
use crate::terminal_host::LaunchIntent;
use agent_domain::CommandId;
use agent_protocol::{
    AgentPreferences,
    submission::{SubmissionInput, SubmissionIntentCall},
};

async fn ready(f: &Fixture, provider: &str) -> ExecutorRef {
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
    probe.actor = ActorRef::SystemRecovery;
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
    ExecutorRef {
        provider_id: agent_domain::ProviderId::parse(provider).unwrap(),
        installation_id: Some(installation.installation_id.to_string()),
    }
}
async fn disable(client: &CoreClient, disabled: Vec<String>) {
    let current = client.agent_preferences().await.unwrap();
    let mut request = CommandEnvelope::local(CoreCommand::SetAgentPreferences(AgentPreferences {
        disabled,
        ..Default::default()
    }));
    request.expected_stream_version = Some(current.revision);
    client.execute(request).await.unwrap();
}
async fn idle(client: &CoreClient) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while client
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .iter()
            .any(|t| t.active_primary_run.is_some())
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
fn input(prompt: &str) -> SubmissionInput {
    SubmissionInput {
        prompt: prompt.into(),
        model: None,
        thinking: None,
        permission_mode: None,
        attachments: vec![],
    }
}
async fn read_until(client: &CoreClient, id: &str, needle: &str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (mut text, mut cursor) = (String::new(), 0);
        loop {
            let batch = client.terminal_read(id, cursor).await.unwrap();
            cursor = batch.next;
            text.push_str(&String::from_utf8_lossy(&batch.bytes));
            if text.contains(needle) {
                return text;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn a_second_protocol_client_drives_daily_work_and_shares_state_with_the_desktop() {
    let f = fixture().await;
    let agent = ready(&f, "fake").await;
    f.core
        .register_driver(Arc::new(FakeDriver::with_executor(agent.clone())))
        .await;
    let desktop = &f.client;
    let phone = CoreClient::Remote(f.directory.path().into());
    let folder = f.directory.path().join("workspace");
    std::fs::create_dir(&folder).unwrap();

    // Add an existing PC folder as a project; the service validates and owns the record.
    let project = phone
        .register_project(ProjectRegistration {
            display_name: String::new(),
            path: folder.to_str().unwrap().into(),
            additional_directories: vec![],
        })
        .await
        .unwrap()
        .project_id;
    assert!(
        desktop
            .bootstrap()
            .await
            .unwrap()
            .projects
            .iter()
            .any(|p| p.project_id == project)
    );

    // Start a new conversation from the phone; the service creates it and sends.
    let started = phone
        .submission_intents(SubmissionIntentCall::Start {
            operation_id: CommandId::new(),
            scope: "phone:draft".into(),
            task_id: None,
            project_id: project.clone(),
            target: agent.clone(),
            input: Box::new(input("from the phone")),
        })
        .await
        .unwrap();
    assert_eq!(started["resolution"]["status"], "submitted", "{started}");
    let task = TaskId::parse(started["resolution"]["taskId"].as_str().unwrap()).unwrap();
    idle(desktop).await;
    assert!(
        desktop
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .iter()
            .any(|t| t.task_id == task)
    );

    // A setting changed on the desktop is the phone's state too.
    disable(desktop, vec!["fake".into()]).await;
    assert_eq!(
        phone
            .agent_preferences()
            .await
            .unwrap()
            .preferences
            .disabled,
        vec!["fake".to_string()]
    );

    // The phone's next send is stored but cannot be admitted while the Agent is disabled.
    let operation = CommandId::new();
    let refused = phone
        .submission_intents(SubmissionIntentCall::Start {
            operation_id: operation.clone(),
            scope: format!("phone:task:{task}"),
            task_id: Some(task.clone()),
            project_id: project.clone(),
            target: agent.clone(),
            input: Box::new(input("second message")),
        })
        .await
        .unwrap_err();
    assert!(refused.contains("禁用"), "{refused}");

    // The desktop discovers the phone's pending original and finishes it once allowed.
    let pending = desktop
        .submission_intents(SubmissionIntentCall::List {
            after: None,
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(pending["operationIds"], json!([operation.to_string()]));
    disable(desktop, vec![]).await;
    let continued = desktop
        .submission_intents(SubmissionIntentCall::Continue {
            operation_id: operation.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        continued["resolution"]["status"], "submitted",
        "{continued}"
    );
    let seen = phone
        .submission_intents(SubmissionIntentCall::Get {
            operation_id: operation,
            expected: None,
        })
        .await
        .unwrap();
    assert_eq!(seen, continued);
    idle(desktop).await;

    // A terminal started from the phone is visible, readable and sized for both clients.
    let terminal = phone
        .terminal_start(LaunchIntent {
            project_launch: None,
            operation_id: uuid::Uuid::new_v4().to_string(),
            project_id: project.to_string(),
            directory_index: 0,
            task_id: None,
            command_id: None,
            label: "phone shell".into(),
            cwd: folder.canonicalize().unwrap(),
            command: Some("printf 'READY\\n'; read line; stty size; printf 'DONE\\n'".into()),
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();
    let listed = desktop.terminal_list(project.as_str()).await.unwrap();
    assert!(listed.iter().any(|t| t.id == terminal.id));
    read_until(desktop, &terminal.id, "READY").await;
    phone.terminal_resize(&terminal.id, 120, 40).await.unwrap();
    let info = desktop.terminal_get(&terminal.id).await.unwrap();
    assert_eq!((info.cols, info.rows), (120, 40));
    desktop
        .terminal_write(&terminal.id, b"go\n".to_vec())
        .await
        .unwrap();
    assert!(
        read_until(&phone, &terminal.id, "DONE")
            .await
            .contains("40 120")
    );
}

#[tokio::test]
async fn a_project_scoped_subscription_never_carries_other_projects() {
    let f = fixture().await;
    let agent = ready(&f, "fake").await;
    f.core
        .register_driver(Arc::new(FakeDriver::with_executor(agent.clone())))
        .await;
    let desktop = &f.client;
    let phone = CoreClient::Remote(f.directory.path().into());
    let mut projects = vec![];
    for name in ["mine", "other"] {
        let folder = f.directory.path().join(name);
        std::fs::create_dir(&folder).unwrap();
        projects.push(
            desktop
                .register_project(ProjectRegistration {
                    display_name: name.into(),
                    path: folder.to_str().unwrap().into(),
                    additional_directories: vec![],
                })
                .await
                .unwrap()
                .project_id,
        );
    }
    let head = desktop.bootstrap().await.unwrap().last_sequence;
    assert!(
        phone
            .subscribe_project(head, &ProjectId::new())
            .await
            .is_err()
    );
    let mut scoped = phone.subscribe_project(head, &projects[0]).await.unwrap();
    let mut tasks = vec![];
    for (index, project) in projects.iter().enumerate() {
        let started = desktop
            .submission_intents(SubmissionIntentCall::Start {
                operation_id: CommandId::new(),
                scope: format!("draft:{index}"),
                task_id: None,
                project_id: project.clone(),
                target: agent.clone(),
                input: Box::new(input("scoped")),
            })
            .await
            .unwrap();
        tasks.push(
            started["resolution"]["taskId"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        idle(desktop).await;
    }
    disable(desktop, vec!["other-agent".into()]).await;
    let mut seen = scoped.replay.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, scoped.live.recv()).await {
        let done = event.event_type == "preferences.agents.updated";
        seen.push(event);
        if done {
            break;
        }
    }
    let streams: Vec<_> = seen
        .iter()
        .map(|e| (e.stream.stream_type.clone(), e.stream.stream_id.clone()))
        .collect();
    // The task created after subscribing joins the scope; the other project's never appears.
    assert!(streams.iter().any(|(_, id)| id == &tasks[0]), "{streams:?}");
    assert!(
        seen.iter()
            .any(|e| e.event_type == "turn.queued" && e.stream.stream_id == tasks[0])
    );
    assert!(
        !streams
            .iter()
            .any(|(_, id)| id == &tasks[1] || id == projects[1].as_str()),
        "{streams:?}"
    );
    // Host-wide shared settings still reach the scoped client.
    assert!(
        seen.iter()
            .any(|e| e.event_type == "preferences.agents.updated")
    );
}
