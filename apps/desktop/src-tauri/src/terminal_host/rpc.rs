//! Authenticated local-service boundary. Project membership is validated here,
//! independent of desktop window labels or client-side path checks.
use super::{LaunchIntent, TerminalHost};
use agent_core::Core;
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn dispatch(
    core: &Arc<Core>,
    terminals: &TerminalHost,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    match method {
        "terminal_start" => {
            let intent: LaunchIntent = serde_json::from_value(args).map_err(|e| e.to_string())?;
            // Recipe bindings are created only by the validated recipe path.
            if intent.project_launch.is_some() {
                return Err("项目运行请求必须通过项目启动入口发送".into());
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
                .ok_or("项目不存在")?;
            if let Some(task_id) = &intent.task_id
                && !snapshot.tasks.iter().any(|task| {
                    task.task_id.as_str() == task_id && task.project_id == project.project_id
                })
            {
                return Err("会话不属于此项目".into());
            }
            let root = std::iter::once(&project.path)
                .chain(&project.additional_directories)
                .nth(intent.directory_index)
                .ok_or("项目目录不存在")?;
            let root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
            let cwd = std::fs::canonicalize(&intent.cwd).map_err(|e| e.to_string())?;
            if !cwd.starts_with(&root) || !cwd.is_dir() {
                return Err("终端目录不在项目范围内".into());
            }
            Ok(json!(terminals.start(intent).await?))
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
        "terminal_write" => {
            let (id, bytes): (String, Vec<u8>) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            terminals.write(&id, bytes).await?;
            Ok(Value::Null)
        }
        "terminal_resize" => {
            let (id, cols, rows): (String, u16, u16) =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            terminals.resize(&id, cols, rows).await?;
            Ok(Value::Null)
        }
        "terminal_stop" => {
            let id: String = serde_json::from_value(args).map_err(|e| e.to_string())?;
            Ok(json!(terminals.stop(&id).await?))
        }
        _ => Err("unknown terminal operation".into()),
    }
}
