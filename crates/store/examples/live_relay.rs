//! Explicit opt-in real provider acceptance. Uses an isolated temporary project/database.
#[path = "support/installation.rs"]
mod installation;
use agent_core::Core;
use agent_domain::{ExecutorRef, ProjectId, TaskId};
use agent_protocol::{
    CommandEnvelope, ConfirmContextPackage, ContextPackageKind, ContextPreviewRequest,
    ContextStrength, CoreCommand, CreateTask, RegisterProject, SubmitTurn,
};
use agent_store::SqliteStore;
use driver_api::AgentDriver;
use std::{path::Path, sync::Arc, time::Duration};

async fn turn(
    core: &Arc<Core>,
    task: &TaskId,
    prompt: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let snapshot = core.bootstrap().await?;
    let start = snapshot.last_sequence;
    let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        review_fix: None,
        attachments: vec![],
        permission_mode: None,
        thinking: None,
        task_id: task.clone(),
        prompt: prompt.into(),
        model: None,
        executor_override: None,
    }));
    command.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    core.execute(command).await?;
    tokio::time::timeout(Duration::from_mins(3), async {
        loop {
            let events = core.sync_events(start, 10_000).await?;
            if let Some(failed) = events.iter().find(|e| e.event_type == "turn.failed") {
                return Err(format!("turn failed: {}", failed.payload).into());
            }
            if events.iter().any(|e| e.event_type == "approval.requested") {
                return Err(
                    "acceptance needs an interactive approval; no automatic approval granted"
                        .into(),
                );
            }
            if events.iter().any(|e| e.event_type == "turn.completed") {
                return Ok(events
                    .iter()
                    .filter(|e| e.event_type == "message.delta")
                    .filter_map(|e| e.payload["text"].as_str())
                    .collect::<String>());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await?
}

async fn switch(
    core: &Arc<Core>,
    task: &TaskId,
    source: ExecutorRef,
    target: ExecutorRef,
    kind: ContextPackageKind,
    after: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = core.bootstrap().await?;
    let request = ContextPreviewRequest {
        task_id: task.clone(),
        kind,
        strength: ContextStrength::Standard,
        source_executor: source,
        target_executor: target,
        after_sequence: after,
        through_sequence: snapshot.last_sequence,
    };
    let built = core.build_context_preview(request.clone()).await?;
    assert!(!built.package.conversation_text.is_empty());
    let mut command =
        CommandEnvelope::local(CoreCommand::ConfirmContextPackage(ConfirmContextPackage {
            request,
            expected_payload_sha256: built.payload_sha256,
            edited_package: None,
        }));
    command.expected_stream_version = Some(snapshot.tasks[0].stream_version);
    core.execute(command).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: live_relay ABSOLUTE_CODEX_PATH ABSOLUTE_KIMI_PATH (makes three real model turns)".into());
    }
    let directory = tempfile::tempdir()?;
    let workspace = directory.path().join("project");
    std::fs::create_dir(&workspace)?;
    let store = Arc::new(SqliteStore::open(directory.path().join("acceptance.db"))?);
    let core = Arc::new(Core::with_workspace_context(
        store,
        Arc::new(
            agent_workspace::GitWorkspaceContextProvider::discover().ok_or("git unavailable")?,
        ),
    ));
    let version = std::process::Command::new(&args[1])
        .arg("--version")
        .output()?;
    if !version.status.success() {
        return Err("Codex version probe failed".into());
    }
    let version = String::from_utf8(version.stdout)?;
    let version = version
        .trim()
        .strip_prefix("codex-cli ")
        .ok_or("unexpected Codex version output")?;
    let a = Arc::new(codex_driver::CodexDriver::new(
        Path::new(&args[1]),
        installation::identity(&core, "codex").await?,
        version.into(),
    )?);
    let launch = acp_driver::AcpLaunch {
        executable: args[2].clone().into(),
        args: vec!["acp".into()],
    };
    let report = acp_driver::AcpDriver::probe_ready(&launch).await?;
    let b = Arc::new(acp_driver::AcpDriver::new(
        "kimi",
        &installation::identity(&core, "kimi").await?,
        launch,
        report.native_version,
        report.resume_capability,
    )?);
    let ae = a.executor().clone();
    let be = b.executor().clone();
    installation::register(&core, a).await?;
    installation::register(&core, b).await?;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            additional_directories: vec![],
            project_id: project.clone(),
            display_name: "Live relay acceptance".into(),
            path: workspace.to_string_lossy().into_owned(),
        },
    )))
    .await?;
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "Live relay acceptance".into(),
            preferred_executor: ae.clone(),
        },
    )))
    .await?;
    let first = turn(&core, &task, "Acceptance test in this isolated directory. Do not use tools or write files. Remember this conversation-only marker: ORCHID-731. Reply exactly ACK ORCHID-731.").await?;
    assert!(first.contains("ORCHID-731"));
    let checkpoint = core
        .bootstrap()
        .await?
        .session_maps
        .into_iter()
        .find(|s| s.executor == ae)
        .ok_or("missing A session")?
        .last_seen_sequence;
    switch(
        &core,
        &task,
        ae.clone(),
        be.clone(),
        ContextPackageKind::Handoff,
        0,
    )
    .await?;
    let second = turn(&core, &task, "Do not use tools. From the supplied conversation history, repeat the previous agent's conversation-only marker. Also remember a new marker: COBALT-842. Reply with both markers.").await?;
    assert!(
        second.contains("ORCHID-731") && second.contains("COBALT-842"),
        "B missed A: {second}"
    );
    switch(
        &core,
        &task,
        be,
        ae,
        ContextPackageKind::Reconciliation,
        checkpoint,
    )
    .await?;
    let third = turn(&core, &task, "Do not use tools. What new marker did the other agent receive while you were away? Reply with that marker.").await?;
    assert!(third.contains("COBALT-842"), "A missed B: {third}");
    println!(
        "PASS real Codex → Kimi → Codex: conversation-only markers verified; no filesystem clues, no console LLM summaries."
    );
    Ok(())
}
