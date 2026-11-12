//! Authenticated local-service boundary. Project membership is validated here,
//! independent of desktop window labels or client-side path checks.
use super::{LaunchIntent, TerminalHost, TerminalStatus, TerminalStopRequest};
use agent_core::Core;
use agent_domain::TargetId;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const RELEASE_WAIT_ROUND: Duration = Duration::from_millis(50);
const RELEASE_WAIT_ROUNDS: usize = 60;

/// Whether a directory is inside a root, by name alone: both are on another machine.
fn within_remote_root(root: &str, cwd: &std::path::Path) -> bool {
    cwd.components()
        .all(|part| !matches!(part, std::path::Component::ParentDir))
        && cwd.starts_with(root)
}

/// What a shell opened on a host, rather than in a project, is listed under. Project
/// identities never have this prefix, so no project can reach these shells.
fn target_scope(target: &TargetId) -> String {
    format!("target:{}", target.as_str())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetShellRequest {
    target_id: TargetId,
    operation_id: String,
    cols: u16,
    rows: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetReleaseRequest {
    target_id: TargetId,
}

pub(crate) async fn dispatch(
    core: &Arc<Core>,
    terminals: &TerminalHost,
    targets: &crate::execution_targets::TargetHost,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    match method {
        "terminal_start" => {
            let intent: LaunchIntent = serde_json::from_value(args).map_err(|e| e.to_string())?;
            // Recipe bindings are created only by the validated recipe path.
            if intent.project_launch.is_some() {
                return Err("backendText.m1d226441".into());
            }
            // Retry an accepted intent even after project configuration changes.
            // A new intent is always checked against the current project roots.
            if let Some(existing) = terminals.existing(&intent)? {
                return Ok(json!(existing));
            }
            let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
            let project = snapshot
                .projects
                .iter()
                .find(|project| project.project_id.as_str() == intent.project_id)
                .ok_or("backendText.projectMissing")?;
            if let Some(task_id) = &intent.task_id
                && !snapshot.tasks.iter().any(|task| {
                    task.task_id.as_str() == task_id && task.project_id == project.project_id
                })
            {
                return Err("backendText.ma0931a4c".into());
            }
            let root = std::iter::once(&project.path)
                .chain(&project.additional_directories)
                .nth(intent.directory_index)
                .ok_or("backendText.m3149b9e1")?;
            if let Some(target) = &project.target_id {
                if !within_remote_root(root, &intent.cwd) {
                    return Err("backendText.mdfacbea4".into());
                }
                let target = targets.target(target)?;
                return Ok(json!(terminals.start_on(intent, target).await?));
            }
            let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
            let cwd = std::fs::canonicalize(&intent.cwd).map_err(|e| e.to_string())?;
            if !cwd.starts_with(&root) || !cwd.is_dir() {
                return Err("backendText.mdfacbea4".into());
            }
            Ok(json!(terminals.start(intent).await?))
        }
        "terminal_start_on_target" => {
            let request: TargetShellRequest =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            // The directory comes from the connection, never from the caller.
            let (target, home) = targets.shell_place(&request.target_id)?;
            let intent = LaunchIntent {
                project_launch: None,
                operation_id: request.operation_id,
                project_id: target_scope(&request.target_id),
                directory_index: 0,
                task_id: None,
                command_id: None,
                label: "backendText.me2a76ef1".into(),
                cwd: home,
                command: None,
                cols: request.cols,
                rows: request.rows,
            };
            Ok(json!(terminals.start_on(intent, target).await?))
        }
        // After the last shell a person opened on a host closes. Agents reach a host
        // through its projects over this same connection, so a host with projects keeps
        // it, even while their conversations are idle: their agents stay alive on it.
        "terminal_release_target" => {
            let request: TargetReleaseRequest =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let target = request.target_id;
            let snapshot = core.bootstrap().await.map_err(|e| e.to_string())?;
            if snapshot
                .projects
                .iter()
                .any(|project| project.target_id.as_ref() == Some(&target))
            {
                return Ok(json!({ "released": false }));
            }
            // A shell just closed is still stopping. Let it finish, or losing the
            // connection would record it as lost and bring its tab back later.
            let scope = target_scope(&target);
            for _ in 0..RELEASE_WAIT_ROUNDS {
                let shells = terminals.list(&scope)?;
                if shells
                    .iter()
                    .any(|info| info.status.active() && info.status != TerminalStatus::Stopping)
                {
                    return Ok(json!({ "released": false }));
                }
                if !shells
                    .iter()
                    .any(|info| info.status == TerminalStatus::Stopping)
                {
                    break;
                }
                tokio::time::sleep(RELEASE_WAIT_ROUND).await;
            }
            targets.disconnect(core, &target).await;
            Ok(json!({ "released": true }))
        }
        "terminal_launch_intents" => {
            let id = args["operationId"]
                .as_str()
                .ok_or("backendText.mb89dee96")?;
            match args["type"].as_str() {
                Some("get") => Ok(json!(terminals.launch_receipt(id)?)),
                Some("retire") => terminals.retire_launch(id).await,
                _ => Err("backendText.mcc93931d".into()),
            }
        }
        "terminal_list" => {
            let project_id: String = serde_json::from_value(args).map_err(|e| e.to_string())?;
            Ok(json!(terminals.list(&project_id)?))
        }
        "terminal_get" => {
            let id: String = serde_json::from_value(args).map_err(|e| e.to_string())?;
            Ok(json!(terminals.get(&id)?))
        }
        "terminal_read" => {
            let (id, after): (String, u64) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            Ok(json!(terminals.read(&id, after).await?))
        }
        "terminal_resize" => {
            let (id, cols, rows): (String, u16, u16) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            terminals.resize(&id, cols, rows).await?;
            Ok(Value::Null)
        }
        "terminal_input" => {
            let (window, request): (String, Value) =
                serde_json::from_value(args).map_err(|_| "backendText.m9c6917ec")?;
            if window != "main" && !window.starts_with("review-") {
                return Err("backendText.mf86abf3f".into());
            }
            terminals
                .input_operation(&format!("local-window:{window}"), request)
                .await
        }
        "terminal_close" => {
            let target: TerminalStopRequest =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            Ok(json!(
                terminals
                    .close(&target.terminal_id, &target.expected_epoch)
                    .await?
            ))
        }
        "terminal_stop" => {
            let target: TerminalStopRequest =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            Ok(json!(
                terminals
                    .stop(&target.terminal_id, &target.expected_epoch)
                    .await?
            ))
        }
        _ => Err("unknown terminal operation".into()),
    }
}
