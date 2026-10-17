//! Opt-in paid/native review acceptance. All source files and state are disposable.
//! Run: `cargo run -p agent-store --example live_review_workflow -- codex code`
use agent_core::{
    Core,
    turn_review::{ReviewKind, ReviewRequest, ReviewStatus},
};
use agent_domain::{ProjectId, RunId, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn};
use agent_store::SqliteStore;
use agent_workspace::review::{CompareMode, ReviewStore, git::GitReview};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
type Error = Box<dyn std::error::Error>;
#[path = "support/native.rs"]
mod native;
use native::driver;

fn git(root: &Path, args: &[&str]) -> Result<(), Error> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err("fixture git command failed".into());
    }
    Ok(())
}
fn files(kind: ReviewKind) -> Vec<(&'static str, &'static str, &'static str)> {
    match kind {
        ReviewKind::Code => vec![
            (
                "payments/withdraw.py",
                "from policy import normalize_amount\ndef withdraw(balance, raw):\n    amount = normalize_amount(raw)\n    if amount > balance: raise ValueError('insufficient balance')\n    return balance - amount\n",
                "from policy import normalize_amount\ndef withdraw(balance, raw):\n    amount = normalize_amount(raw)\n    return balance - amount\n",
            ),
            (
                "reports/last.py",
                "def last_value(values):\n    if not values: return None\n    return values[-1]\n",
                "def last_value(values):\n    if not values: return None\n    return values[len(values)]\n",
            ),
        ],
        ReviewKind::Security => vec![(
            "documents/download.py",
            "from policy import load_document, ensure_owner\ndef download(user, doc_id):\n    doc = load_document(doc_id)\n    ensure_owner(user, doc)\n    return doc.body\n",
            "from policy import load_document, ensure_owner\ndef download(user, doc_id):\n    doc = load_document(doc_id)\n    return doc.body\n",
        )],
        _ => vec![(
            "reports/names.py",
            "from policy import query_users\ndef names(ids):\n    users = query_users(ids)\n    return [users[id].name for id in ids]\n",
            "from policy import query_users\ndef names(ids):\n    return [query_users([id])[id].name for id in ids]\n",
        )],
    }
}
const POLICY: &str = "def normalize_amount(raw):\n    amount = int(raw)\n    if amount <= 0: raise ValueError('positive amount required')\n    return amount\n\ndef ensure_owner(user, doc):\n    if user.id != doc.owner_id: raise PermissionError('not owner')\n\ndef load_document(doc_id):\n    # No authorization is performed here; the caller must check ownership.\n    return db.get_document(doc_id)\n\ndef query_users(ids):\n    # Each invocation performs one remote SQL query, including singleton input.\n    return db.fetch_users_by_ids(ids)\n";
fn main() -> Result<(), Error> {
    if antigravity_driver::approval_bridge::run_hook_if_requested() {
        return Ok(());
    }
    tokio::runtime::Runtime::new()?.block_on(run())
}

async fn run() -> Result<(), Error> {
    let name = std::env::args()
        .nth(1)
        .ok_or("provider required; this calls a real billed Agent")?;
    let kind = match std::env::args().nth(2).as_deref() {
        Some("code") => ReviewKind::Code,
        Some("security") => ReviewKind::Security,
        Some("performance") => ReviewKind::Performance,
        _ => return Err("kind must be code/security/performance".into()),
    };
    let directory = tempfile::Builder::new()
        .prefix("dogcodes-live-review-workflow-")
        .tempdir()?
        .keep();
    println!("evidence={}", directory.display());
    let project_path = directory.join("project");
    std::fs::create_dir(&project_path)?;
    git(&project_path, &["init", "-q"])?;
    let fixture = files(kind);
    for (path, before, _) in &fixture {
        let file = project_path.join(path);
        std::fs::create_dir_all(file.parent().ok_or("parent absent")?)?;
        std::fs::write(file, before)?;
    }
    std::fs::write(project_path.join("policy.py"), POLICY)?;
    git(&project_path, &["add", "."])?;
    git(
        &project_path,
        &[
            "-c",
            "user.name=Review Fixture",
            "-c",
            "user.email=review@example.invalid",
            "commit",
            "-qm",
            "isolated baseline",
        ],
    )?;
    for (path, _, after) in &fixture {
        std::fs::write(project_path.join(path), after)?;
    }
    let snapshots = ReviewStore::open(directory.join("blobs"))?;
    let git_review = GitReview::default();
    let root = git_review
        .discover(&[project_path.to_string_lossy().into_owned()])
        .await?
        .remove(0);
    let comparison = git_review
        .compare(&root, CompareMode::Uncommitted, None, 0, &snapshots)
        .await?;
    let files: Vec<_> = comparison
        .changes
        .iter()
        .map(|change| json!({"path":change.path,"kind":change.kind}))
        .collect();
    let store = Arc::new(SqliteStore::open(directory.join("core.db"))?);
    let core = Arc::new(Core::new(store.clone()));
    // This isolated harness owns its installation identity; production discovery
    // still performs the real executable/authentication readiness checks.
    let probe = |status, version| {
        let mut command = CommandEnvelope::local(CoreCommand::RecordInstallationProbe(
            agent_protocol::RecordInstallationProbe {
                provider_id: name.clone(),
                executable: format!("native-review-fixture:{name}"),
                fingerprint: "isolated-native-review-harness".into(),
                status,
                version,
                version_text: None,
                diagnostic: Some("Isolated acceptance harness installation".into()),
                elapsed_millis: 0,
            },
        ));
        command.actor = agent_protocol::ActorRef::SystemRecovery;
        command
    };
    core.execute(probe(agent_protocol::InstallationStatus::Installed, None))
        .await?;
    let installation = core
        .bootstrap()
        .await?
        .installations
        .into_iter()
        .find(|installation| installation.provider_id == name)
        .ok_or("fixture installation missing")?;
    let d = tokio::time::timeout(
        Duration::from_mins(1),
        driver(&name, &installation.installation_id.to_string()),
    )
    .await??;
    // Optional native startup diagnosis: no model prompt is submitted.
    if std::env::args().any(|arg| arg == "--session-probe") {
        let isolated = tempfile::tempdir()?;
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_mins(1),
            d.create_analysis_session(&isolated.path().to_string_lossy()),
        )
        .await;
        println!("session_probe_ms={}", started.elapsed().as_millis());
        let handle = result??;
        println!("session_probe=ready");
        tokio::time::timeout(Duration::from_secs(5), d.shutdown(handle)).await??;
        return Ok(());
    }
    let executor = d.executor().clone();
    let native_version = d.native_agent_version().map(str::to_owned);
    core.execute(probe(
        agent_protocol::InstallationStatus::Ready,
        native_version.clone(),
    ))
    .await?;
    core.register_driver(d).await;
    let project_id = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project_id.clone(),
            display_name: "Isolated real review".into(),
            path: project_path.to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await?;
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id,
            title: "Native review acceptance".into(),
            preferred_executor: executor.clone(),
        },
    )))
    .await?;
    let request = ReviewRequest {
        recheck_of: None,
        task_id: task.clone(),
        turn_id: None,
        source_snapshot: Some("native-fixture".into()),
        run_id: RunId::new(),
        executor,
        kind,
        retry_of: None,
    };
    let packet = json!({"taskId":task,"snapshotId":"native-fixture","scope":"isolated Git working tree","roots":[{"rootId":root.id,"path":root.path,"git":true,"scopes":root.scopes,"mode":"uncommitted","base":comparison.base,"target":comparison.target,"files":files}]});
    std::fs::write(
        directory.join("input.json"),
        serde_json::to_vec_pretty(&packet)?,
    )?;
    core.start_project_review(request.clone(), Some(packet))
        .await?;
    let mut previous = String::new();
    let finished = tokio::time::timeout(Duration::from_mins(16), async {
        loop {
            let run = core
                .turn_reviews(&task)
                .await?
                .into_iter()
                .find(|r| r.request.run_id == request.run_id)
                .ok_or("run absent")?;
            let progress = format!(
                "{:?}: {:?}",
                run.status,
                run.workflow.as_ref().map(|w| w
                    .groups
                    .iter()
                    .map(|g| format!(
                        "{}={} requests={} sessions={}",
                        g.id,
                        g.status,
                        g.context_requests,
                        g.sessions.len()
                    ))
                    .collect::<Vec<_>>())
            );
            if progress != previous {
                println!("{name}: {progress}");
                previous = progress;
            }
            if !matches!(
                run.status,
                ReviewStatus::Queued | ReviewStatus::Running | ReviewStatus::Cancelling
            ) {
                return Ok::<_, Error>(run);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await;
    let run = match finished {
        Ok(result) => result?,
        Err(e) => {
            core.cancel_turn_review(&task, &request.run_id).await?;
            return Err(e.into());
        }
    };
    std::fs::write(directory.join("run.json"), serde_json::to_vec_pretty(&run)?)?;
    store.rebuild_projections()?;
    let restored = Core::new(store.clone()).turn_reviews(&task).await?;
    let summary = json!({"provider":name,"nativeVersion":native_version,"kind":kind,"status":run.status,"error":run.error,"workspaceReview":run.workspace_review,"workflowAbsent":run.workflow.is_none(),"report":run.report,"restored":restored.first().is_some_and(|r|r.input_sha256==run.input_sha256),"fixtureUnchanged":fixture.iter().all(|(p,_,after)|std::fs::read_to_string(project_path.join(p)).ok().as_deref()==Some(*after))});
    std::fs::write(
        directory.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    if name == "codex" && kind == ReviewKind::Code && run.status == ReviewStatus::Completed {
        let boot = core.bootstrap().await?;
        let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {review_fix: None, task_id:task.clone(),prompt:"这是隔离验收。不要调用工具、修改文件或进行新的审查。根据上方 readonly_review 来源材料，用中文简短列出已返回的问题；材料中没有的内容不要推测。".into(),model:None,thinking:None,permission_mode:None,executor_override:None,attachments:vec![]}));
        command.expected_stream_version = Some(
            boot.tasks
                .iter()
                .find(|t| t.task_id == task)
                .ok_or("task absent")?
                .stream_version,
        );
        core.execute(command).await?;
        tokio::time::timeout(Duration::from_mins(3), async {
            loop {
                let events = core.sync_events(boot.last_sequence, 10_000).await?;
                if events.iter().any(|e| {
                    matches!(
                        e.event_type.as_str(),
                        "turn.completed" | "turn.failed" | "approval.requested"
                    )
                }) {
                    let text: String = events
                        .iter()
                        .filter(|e| e.event_type == "message.delta")
                        .filter_map(|e| e.payload["text"].as_str())
                        .collect();
                    std::fs::write(directory.join("primary-return.txt"), &text)?;
                    println!(
                        "primary_return_completed={} bytes={}",
                        events.iter().any(|e| e.event_type == "turn.completed"),
                        text.len()
                    );
                    return Ok::<_, Error>(());
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
        .await??;
    }
    if run.status != ReviewStatus::Completed {
        return Err("real review did not complete; see saved summary.json".into());
    }
    Ok(())
}
