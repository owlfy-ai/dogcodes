use super::*;
use crate::review_repairs::{RepairCall, RepairEntry, RepairState, submit_envelope};
use agent_core::turn_review::{ReviewKind, ReviewRequest};
use agent_domain::CommandId;
use agent_protocol::{CreateTask, ReviewFixRef, SubmitTurn};
use driver_api::AgentDriver;

const REPORT: &str = r#"{"summary":"检查完成","findings":[{"root":"/repo","path":"file","severity":"high","title":"first","description":"reason","side":"after","startLine":1,"endLine":1},{"root":"/repo","path":"file","severity":"low","title":"second","description":"reason","side":"after","startLine":1,"endLine":1}]}"#;

/// A read-only reviewer whose final answer is a fixed two-finding report.
struct Reviewer {
    executor: ExecutorRef,
    fake: FakeDriver,
}
#[async_trait::async_trait]
impl AgentDriver for Reviewer {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    async fn create_review_session(
        &self,
        directory: &str,
        additional: &[String],
    ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
        AgentDriver::create_review_session(&self.fake, directory, additional).await
    }
    async fn create_session(
        &self,
        _: driver_api::SessionRequest,
    ) -> Result<driver_api::SessionHandle, driver_api::DriverError> {
        panic!("reviewer must not start a coding session")
    }
    async fn prompt(
        &self,
        handle: &driver_api::SessionHandle,
        mut request: driver_api::PromptRequest,
    ) -> Result<driver_api::DriverEventStream, driver_api::DriverError> {
        // The fake echoes the prompt after one prose line; one fenced JSON block is accepted.
        request.prompt = format!("\n```json\n{REPORT}\n```");
        AgentDriver::prompt(&self.fake, handle, request).await
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

struct Setup {
    f: Fixture,
    task: TaskId,
    primary: Arc<FakeDriver>,
    other: Arc<FakeDriver>,
    reference: ReviewFixRef,
}

fn git(path: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn idle(f: &Fixture, task: &TaskId) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while f
            .core
            .bootstrap()
            .await
            .unwrap()
            .tasks
            .iter()
            .any(|t| &t.task_id == task && t.active_primary_run.is_some())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// A git project, a task on `fake`, and a completed review with two findings.
async fn setup() -> Setup {
    let f = fixture().await;
    let primary = Arc::new(FakeDriver::with_executor(ready(&f, "fake").await));
    let other = Arc::new(FakeDriver::with_executor(ready(&f, "other").await));
    let reviewer = Arc::new(Reviewer {
        executor: ready(&f, "reviewer").await,
        fake: FakeDriver::new("reviewer").unwrap(),
    });
    for driver in [primary.clone(), other.clone()] {
        f.core.register_driver(driver).await;
    }
    f.core.register_driver(reviewer.clone()).await;
    let path = f.directory.path().join("workspace");
    std::fs::create_dir(&path).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Test"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "commit.gpgsign", "false"],
    ] {
        git(&path, &args);
    }
    std::fs::write(path.join("file"), "before\n").unwrap();
    git(&path, &["add", "."]);
    git(&path, &["commit", "-qm", "base"]);
    std::fs::write(path.join("file"), "after\n").unwrap();
    let project = ProjectId::new();
    let task = TaskId::new();
    f.core
        .execute(CommandEnvelope::local(CoreCommand::RegisterProject(
            RegisterProject {
                project_id: project.clone(),
                display_name: "repair".into(),
                path: path.to_string_lossy().into(),
                additional_directories: vec![],
            },
        )))
        .await
        .unwrap();
    f.core
        .execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: task.clone(),
                project_id: project,
                title: "repair".into(),
                preferred_executor: primary.executor().clone(),
            },
        )))
        .await
        .unwrap();
    let snapshot: crate::review::ReviewSnapshot = f
        .client
        .review(
            serde_json::from_value(
                json!({"operation":"query","arguments":{"taskId":task,"mode":"uncommitted"}}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let snapshot = serde_json::to_value(snapshot).unwrap();
    let run = f
        .client
        .start_project_review(ReviewRequest {
            task_id: task.clone(),
            turn_id: None,
            source_snapshot: Some(snapshot["id"].as_str().unwrap().into()),
            run_id: RunId::new(),
            executor: reviewer.executor.clone(),
            kind: ReviewKind::Code,
            retry_of: None,
            recheck_of: None,
        })
        .await
        .unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let runs = f.client.turn_reviews(&task).await.unwrap();
            let current = runs
                .into_iter()
                .find(|r| r.request.run_id == run.request.run_id)
                .unwrap();
            let status = serde_json::to_value(current.status).unwrap();
            if status != "queued" && status != "running" {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(completed.status).unwrap(), "completed");
    let reference = ReviewFixRef {
        run_id: completed.request.run_id.clone(),
        input_sha256: completed.input_sha256.clone(),
        finding_indices: vec![1, 0, 1],
    };
    Setup {
        f,
        task,
        primary,
        other,
        reference,
    }
}

async fn call(client: &CoreClient, call: RepairCall) -> Result<Option<RepairEntry>, String> {
    serde_json::from_value(client.review_repair(call).await?).map_err(|e| e.to_string())
}
fn start(s: &Setup, operation: &CommandId, target: Option<ExecutorRef>) -> RepairCall {
    RepairCall::Start {
        operation_id: operation.clone(),
        task_id: s.task.clone(),
        reference: s.reference.clone(),
        target,
    }
}
fn get(s: &Setup) -> RepairCall {
    RepairCall::Get {
        task_id: s.task.clone(),
        run_id: s.reference.run_id.clone(),
    }
}
fn submitted(entry: &RepairEntry) -> bool {
    matches!(entry.state, RepairState::Submitted { .. })
}

#[tokio::test]
async fn paired_repair_keeps_its_actor_and_old_records_default_to_local() {
    let s = setup().await;
    let actor = ActorRef::PairedDevice(agent_domain::DeviceId::new());
    let operation = CommandId::new();
    let value =
        s.f.control
            .repairs
            .call_as(&s.f.core, start(&s, &operation, None), actor.clone())
            .await
            .unwrap();
    let entry: RepairEntry = serde_json::from_value(value).unwrap();
    assert_eq!(entry.intent.actor, actor);
    assert_eq!(
        submit_envelope(&entry.intent, entry.intent.base_version).actor,
        actor
    );
    let persisted = call(&s.f.client, get(&s)).await.unwrap().unwrap();
    assert_eq!(persisted.intent.actor, actor);
    let mut legacy = serde_json::to_value(entry).unwrap();
    legacy["intent"].as_object_mut().unwrap().remove("actor");
    assert_eq!(
        serde_json::from_value::<RepairEntry>(legacy)
            .unwrap()
            .intent
            .actor,
        ActorRef::LocalUser
    );
    idle(&s.f, &s.task).await;
}

#[tokio::test]
async fn repair_is_driven_by_the_service_and_a_retried_start_does_not_resubmit() {
    let s = setup().await;
    let operation = CommandId::new();
    let entry = call(&s.f.client, start(&s, &operation, None))
        .await
        .unwrap()
        .unwrap();
    assert!(submitted(&entry));
    assert_eq!(entry.intent.reference.finding_indices, vec![0, 1]);
    assert!(entry.intent.context.is_none());
    idle(&s.f, &s.task).await;
    let requests = s.primary.received_requests().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0].prompt.contains("first") && requests[0].prompt.contains("second"));

    // Another client sees the same record and a retry with the same identity is a no-op.
    let other = CoreClient::Remote(s.f.directory.path().into());
    assert_eq!(call(&other, get(&s)).await.unwrap(), Some(entry.clone()));
    let retried = call(&other, start(&s, &operation, None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retried, entry);
    assert_eq!(s.primary.received_requests().await.len(), 1);
    // The same identity cannot be reused for different findings.
    let mut changed = start(&s, &operation, None);
    if let RepairCall::Start { reference, .. } = &mut changed {
        reference.finding_indices = vec![0];
    }
    assert!(
        call(&other, changed)
            .await
            .unwrap_err()
            .contains("不同的修复内容")
    );
}

#[tokio::test]
async fn frozen_agent_switch_is_visible_to_other_clients_and_resumed_once() {
    let s = setup().await;
    let operation = CommandId::new();
    // Simulate a client that froze the request and disappeared before any mutation.
    let frozen =
        s.f.control
            .repairs
            .freeze(
                &s.f.core,
                operation.clone(),
                s.task.clone(),
                s.reference.clone(),
                Some(s.other.executor().clone()),
            )
            .await
            .unwrap();
    assert!(frozen.intent.context.is_some());
    let other = CoreClient::Remote(s.f.directory.path().into());
    let discovered = call(&other, get(&s)).await.unwrap().unwrap();
    assert_eq!(discovered, frozen);
    // A different request for the same review run must wait for the original.
    assert!(
        call(&other, start(&s, &CommandId::new(), None))
            .await
            .unwrap_err()
            .contains("请先核实原修复请求")
    );
    let resumed = call(
        &other,
        RepairCall::Resume {
            task_id: s.task.clone(),
            run_id: s.reference.run_id.clone(),
            operation_id: operation.clone(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(submitted(&resumed));
    idle(&s.f, &s.task).await;
    let task = s.f.core.bootstrap().await.unwrap().tasks.remove(0);
    assert_eq!(&task.preferred_executor, s.other.executor());
    assert_eq!(s.other.received_requests().await.len(), 1);
    assert!(s.primary.received_requests().await.is_empty());
    // Resuming a finished record is read-only.
    let again = call(
        &s.f.client,
        RepairCall::Resume {
            task_id: s.task.clone(),
            run_id: s.reference.run_id.clone(),
            operation_id: operation,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(again, resumed);
    assert_eq!(s.other.received_requests().await.len(), 1);
}

#[tokio::test]
async fn busy_task_is_rejected_before_recording_and_abandon_fences_the_original_submit() {
    let s = setup().await;
    // Hold execution preparation so an unrelated turn keeps the task busy.
    let preparation = s.f.core.try_workspace_mutation().unwrap();
    let mut busy = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: s.task.clone(),
        prompt: "other work".into(),
        executor_override: None,
        attachments: vec![],
        model: None,
        thinking: None,
        permission_mode: None,
        review_fix: None,
    }));
    busy.expected_stream_version =
        Some(s.f.core.bootstrap().await.unwrap().tasks[0].stream_version);
    s.f.client.execute(busy).await.unwrap();
    assert!(
        call(&s.f.client, start(&s, &CommandId::new(), None))
            .await
            .unwrap_err()
            .contains("请等待主对话")
    );
    assert_eq!(call(&s.f.client, get(&s)).await.unwrap(), None);
    drop(preparation);
    idle(&s.f, &s.task).await;

    let operation = CommandId::new();
    let frozen =
        s.f.control
            .repairs
            .freeze(
                &s.f.core,
                operation.clone(),
                s.task.clone(),
                s.reference.clone(),
                None,
            )
            .await
            .unwrap();
    let abandoned = call(
        &s.f.client,
        RepairCall::Abandon {
            task_id: s.task.clone(),
            run_id: s.reference.run_id.clone(),
            operation_id: operation,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(abandoned.state, RepairState::Abandoned);
    // A late delivery of the exact original command can no longer be accepted.
    assert!(
        s.f.core
            .execute(submit_envelope(&frozen.intent, frozen.intent.base_version))
            .await
            .is_err()
    );
    let before = s.primary.received_requests().await.len();
    // A new request is allowed after the original was closed.
    let next = call(&s.f.client, start(&s, &CommandId::new(), None))
        .await
        .unwrap()
        .unwrap();
    assert!(submitted(&next));
    idle(&s.f, &s.task).await;
    assert_eq!(s.primary.received_requests().await.len(), before + 1);
}
