//! Opt-in real Codex/Core lifecycle check; controlled file edits are made by
//! this fixture, not attributed to the model. Never prepares the baseline itself.
#[path = "support/installation.rs"]
mod installation;
use agent_core::Core;
use agent_domain::{ProjectId, TaskId};
use agent_protocol::{
    CancelTurn, CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn,
};
use agent_store::SqliteStore;
use agent_workspace::{
    GitWorkspaceContextProvider,
    review::{ReviewStore, SnapshotBudget, compare_snapshots},
};
use driver_api::AgentDriver;
use std::{path::Path, sync::Arc, time::Duration};

async fn turn(core: &Arc<Core>, task: &TaskId) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = core.bootstrap().await?;
    let start = snapshot.last_sequence;
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
                        attachments: vec![],
            permission_mode: None,
            thinking: None,
        task_id: task.clone(), prompt: "Isolated integration test. Do not use tools or modify any files. Reply exactly REVIEW_BASELINE_OK.".into(), model: None, executor_override: None,
    }));
    command.expected_stream_version = Some(
        snapshot
            .tasks
            .iter()
            .find(|entry| entry.task_id == *task)
            .ok_or("task missing")?
            .stream_version,
    );
    core.execute(command).await?;
    let result = tokio::time::timeout(Duration::from_mins(2), async {
        loop {
            let events = core.sync_events(start, 10000).await?;
            if events.iter().any(|event| {
                event.event_type == "turn.failed" || event.event_type == "approval.requested"
            }) {
                return Err::<(), Box<dyn std::error::Error>>(
                    "real turn failed or unexpectedly requested approval".into(),
                );
            }
            if events
                .iter()
                .any(|event| event.event_type == "turn.completed")
            {
                let text: String = events
                    .iter()
                    .filter(|event| event.event_type == "message.delta")
                    .filter_map(|event| event.payload["text"].as_str())
                    .collect();
                if !text.contains("REVIEW_BASELINE_OK") {
                    return Err("real reply marker missing".into());
                }
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    if !matches!(&result, Ok(Ok(()))) {
        let snapshot = core.bootstrap().await?;
        if let Some(active) = snapshot
            .tasks
            .iter()
            .find(|entry| entry.task_id == *task)
            .and_then(|entry| entry.active_primary_run.as_ref())
        {
            let mut cancel = CommandEnvelope::local(CoreCommand::CancelTurn(CancelTurn {
                task_id: task.clone(),
                turn_id: active.turn_id.clone(),
                run_id: active.run_id.clone(),
            }));
            cancel.expected_stream_version = Some(
                snapshot
                    .tasks
                    .iter()
                    .find(|entry| entry.task_id == *task)
                    .ok_or("task missing")?
                    .stream_version,
            );
            let _ = core.execute(cancel).await;
        }
    }
    result?
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::args()
        .nth(1)
        .ok_or("usage: live_review_baseline ABSOLUTE_CODEX_PATH (two real turns)")?;
    if !Path::new(&executable).is_absolute() {
        return Err("absolute executable required".into());
    }
    let directory = tempfile::Builder::new()
        .prefix("dogcodes-live-review-")
        .tempdir()?
        .keep();
    println!("isolated evidence directory: {}", directory.display());
    let workspace = directory.join("project");
    std::fs::create_dir(&workspace)?;
    std::fs::write(workspace.join("sample.txt"), b"before first turn\n")?;
    let review_path = directory.join("review");
    let review = ReviewStore::open(review_path.clone())?;
    let core = Arc::new(Core::with_workspace_context(
        Arc::new(SqliteStore::open(directory.join("core.db"))?),
        Arc::new(
            GitWorkspaceContextProvider::discover()
                .ok_or("git unavailable")?
                .with_review_directory(review_path.clone()),
        ),
    ));
    let version = std::process::Command::new(&executable)
        .arg("--version")
        .output()?;
    if !version.status.success() {
        return Err("version probe failed".into());
    }
    let version = String::from_utf8(version.stdout)?;
    let driver = Arc::new(codex_driver::CodexDriver::new(
        Path::new(&executable),
        installation::identity(&core, "codex").await?,
        version
            .trim()
            .strip_prefix("codex-cli ")
            .ok_or("unexpected version")?
            .into(),
    )?);
    let executor = driver.executor().clone();
    installation::register(&core, driver).await?;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "Live Review baseline".into(),
            path: workspace.to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await?;
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "Real lifecycle baseline".into(),
            preferred_executor: executor,
        },
    )))
    .await?;
    assert!(review.load_baseline(&task.to_string(), &workspace).is_err());
    turn(&core, &task).await?;
    let first = review.load_baseline(&task.to_string(), &workspace)?;
    let hash = first
        .files
        .get("sample.txt")
        .ok_or("baseline sample missing")?
        .hash
        .clone();
    assert_eq!(review.read_blob(&hash)?, b"before first turn\n");
    println!("PASS first real Core turn automatically established baseline");
    std::fs::write(workspace.join("sample.txt"), b"controlled later edit\n")?;
    turn(&core, &task).await?;
    drop(review);
    let review = ReviewStore::open(review_path)?;
    let baseline = review.load_baseline(&task.to_string(), &workspace)?;
    assert_eq!(
        baseline
            .files
            .get("sample.txt")
            .ok_or("reopened sample missing")?
            .hash,
        hash
    );
    let current = review.scan(&workspace, SnapshotBudget::default())?;
    let changes = compare_snapshots(&baseline, &current)?;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "sample.txt");
    assert!(!workspace.join(".git").exists());
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":true,"taskId":task,"realTurns":2,"modelFileEdits":false,"checks":["baseline absent before dispatch","first real turn establishes original baseline","second real turn preserves baseline","reopened diff compares original with controlled later edit","non-Git workspace remains non-Git"]}),
        )?,
    )?;
    println!("PASS second real turn and reopened store preserve original baseline");
    Ok(())
}
