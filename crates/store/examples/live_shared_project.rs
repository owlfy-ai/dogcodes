//! Opt-in native shared-directory acceptance. No user workspace is modified.
//! Usage: `live_shared_project codex|kimi [cancel]`
#[path = "support/installation.rs"]
mod installation;
#[path = "support/native.rs"]
mod native;
use agent_core::Core;
use agent_domain::{ProjectId, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn};
use agent_store::SqliteStore;
use serde_json::json;
use std::{sync::Arc, time::Duration};
type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    if agent_core::history_bridge::run_query_if_requested() {
        return Ok(());
    }
    tokio::runtime::Runtime::new()?.block_on(run())
}
async fn run() -> Result<(), Error> {
    let provider = std::env::args()
        .nth(1)
        .ok_or("provider required; calls real paid agents")?;
    if !["codex", "kimi"].contains(&provider.as_str()) {
        return Err("expected codex or kimi".into());
    }
    let cancel = std::env::args().nth(2).as_deref() == Some("cancel");
    let directory = tempfile::Builder::new()
        .prefix("dogcodes-live-shared-")
        .tempdir()?
        .keep();
    println!("evidence={}", directory.display());
    let root = directory.join("project");
    std::fs::create_dir(&root)?;
    for name in ["a", "b"] {
        std::fs::write(root.join(format!("{name}.txt")), format!("BEFORE_{name}\n"))?;
    }
    let core = Arc::new(
        Core::with_workspace_context(
            Arc::new(SqliteStore::open(directory.join("state.db"))?),
            Arc::new(agent_workspace::GitWorkspaceContextProvider::default()),
        )
        .with_history_query_program(std::env::current_exe()?),
    );
    let a = native::driver("codex", &installation::identity(&core, "codex").await?).await?;
    let ae = a.executor().clone();
    installation::register(&core, a).await?;
    let be = if provider == "codex" {
        ae.clone()
    } else {
        let b = native::driver(&provider, &installation::identity(&core, &provider).await?).await?;
        let e = b.executor().clone();
        installation::register(&core, b).await?;
        e
    };
    let project = ProjectId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "Shared native acceptance".into(),
            path: root.to_string_lossy().into_owned(),
            additional_directories: vec![],
        },
    )))
    .await?;
    let tasks = [TaskId::new(), TaskId::new()];
    for (index, executor) in [ae, be].into_iter().enumerate() {
        core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
            CreateTask {
                task_id: tasks[index].clone(),
                project_id: project.clone(),
                title: format!("Shared {index}"),
                preferred_executor: executor,
            },
        )))
        .await?;
    }
    for (index, task) in tasks.iter().enumerate() {
        let name = ["a", "b"][index];
        let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            review_fix: None,
            task_id: task.clone(),
            prompt: format!(
                "Isolated acceptance fixture. Replace ONLY {name}.txt with exactly AFTER_{name} followed by newline. Use apply_patch or native file edit/write tools; no shell, no other files. Respect approvals; do not bypass denials. Reply only DONE_{name} after successful edit."
            ),
            attachments: vec![],
            permission_mode: None,
            thinking: None,
            model: None,
            executor_override: None,
        }));
        command.expected_stream_version = core
            .bootstrap()
            .await?
            .tasks
            .iter()
            .find(|t| &t.task_id == task)
            .map(|t| t.stream_version);
        core.execute(command).await?;
    }
    let mut overlap = false;
    let mut cancelled = false;
    let mut approvals = std::collections::BTreeSet::new();
    tokio::time::timeout(Duration::from_mins(5), async {
        loop {
            let snapshot = core.bootstrap().await?;
            overlap |= tasks.iter().all(|id| snapshot.tasks.iter().any(|t| &t.task_id == id && t.active_primary_run.is_some()));
            // Hold both approvals until both independent requests are visible.
            let pa = core.pending_approvals(&tasks[0], 20).await?;
            let pb = core.pending_approvals(&tasks[1], 20).await?;
            if (!pa.is_empty() && !pb.is_empty()) || !approvals.is_empty() || cancelled {
                for (index, pending) in [pa, pb].into_iter().enumerate() {
                    for approval in pending {
                        if cancel && index == 0 && !cancelled {
                            let task = core.bootstrap().await?.tasks.into_iter().find(|t|t.task_id == tasks[0]).ok_or("missing task")?;
                            let active = task.active_primary_run.ok_or("missing active run")?;
                            let mut cmd = CommandEnvelope::local(CoreCommand::CancelTurn(agent_protocol::CancelTurn {task_id:tasks[0].clone(),turn_id:active.turn_id,run_id:active.run_id}));
                            cmd.expected_stream_version=Some(task.stream_version);
                            core.execute(cmd).await?; cancelled=true; continue;
                        }
                        if cancel && index == 0 {continue;}
                        let name = ["a", "b"][index];
                        let summary: serde_json::Value = serde_json::from_str(&approval.command_summary)?;
                        println!("approval task={index} tool={} summary={summary}", approval.tool_name);
                        let summary = summary.get("input").unwrap_or(&summary);
                        let files = summary["files"].as_array();
                        let codex_exact = approval.tool_name == "fileChange" && summary["scope"] == "reported_files" && summary["detailsSource"] == "matching_item_started" && summary["grantRoot"].is_null()
                            && files.is_some_and(|files| files.len()==1 && files[0]["path"]["path"] == format!("{name}.txt") && files[0]["path"]["relativeTo"] == "current_working_directory" && files[0]["kind"]["movePath"].is_null() && ((files[0]["kind"]["type"] == "update" && files[0]["diff"] == format!("@@ -1 +1 @@\n-BEFORE_{name}\n+AFTER_{name}\n")) || (files[0]["kind"]["type"] == "add" && files[0]["diff"] == format!("AFTER_{name}\n"))));
                        let kimi_fixture = provider == "kimi" && index == 1 && approval.tool_name == "Write" && summary == &json!({"title":"Write","content":[{"type":"content","content":{"type":"text","text":"Requesting approval to Writing b.txt"}}]});
                        if !codex_exact && !kimi_fixture { return Err::<(),Error>("unexpected approval; inspect evidence; no approval granted".into()); }
                        let option = approval.options.iter().find(|o|o.semantic_role == "approve_once").ok_or("no approve_once option")?;
                        let mut cmd = CommandEnvelope::local(CoreCommand::RespondToApproval(agent_protocol::RespondToApproval {
                            turn_id:approval.turn_id.clone(),requested_at_sequence:approval.requested_at_sequence,
                            task_id:tasks[index].clone(),approval_id:approval.approval_id.clone(),option_id:option.option_id.clone(),answer:None,
                        }));
                        cmd.expected_stream_version=core.bootstrap().await?.tasks.iter().find(|t|t.task_id==tasks[index]).map(|t|t.stream_version);
                        core.execute(cmd).await?;
                        approvals.insert((index,approval.approval_id));
                    }
                }
            }
            if tasks.iter().all(|id| snapshot.tasks.iter().any(|t| &t.task_id==id && t.active_primary_run.is_none())) {break;}
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok::<(),Error>(())
    }).await??;
    let snapshot = core.bootstrap().await?;
    let events = core.sync_events(0, 10_000).await?;
    std::fs::write(
        directory.join("events.json"),
        serde_json::to_vec_pretty(&events)?,
    )?;
    std::fs::write(
        directory.join("snapshot.json"),
        serde_json::to_vec_pretty(&snapshot)?,
    )?;
    assert!(overlap, "turns never overlapped");
    assert_eq!(cancel, cancelled);
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt"))?,
        if cancel { "BEFORE_a\n" } else { "AFTER_a\n" }
    );
    assert_eq!(std::fs::read_to_string(root.join("b.txt"))?, "AFTER_b\n");
    assert_eq!(snapshot.session_maps.len(), 2);
    assert_ne!(
        snapshot.session_maps[0].native_session_id,
        snapshot.session_maps[1].native_session_id
    );
    for (index, task) in tasks.iter().enumerate() {
        let text = core
            .timeline(task)
            .await?
            .into_iter()
            .map(|i| i.text)
            .collect::<String>();
        assert!(
            !text.contains(if index == 0 { "DONE_b" } else { "DONE_a" }),
            "cross-task message leak"
        );
    }
    let report = json!({"passed":true,"providerB":provider,"sameProject":true,"overlap":overlap,"cancelledA":cancelled,"approvals":approvals.len(),"independentSessions":true});
    std::fs::write(
        directory.join("summary.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{report}");
    Ok(())
}
