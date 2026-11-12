//! Desktop adapter for service-owned terminals. Reading output never owns a PTY.
use crate::{
    DesktopState,
    terminal_host::{LaunchIntent, OutputBatch, TerminalInfo},
};

fn authorize(label: &str) -> Result<(), String> {
    // This is a desktop entry-point restriction, not resource ownership. The
    // authenticated service validates business scope independently of windows.
    if label == "main" || label.starts_with("review-") {
        Ok(())
    } else {
        Err("backendText.m34f6bae8".into())
    }
}

fn allows_task(label: &str, task_id: Option<&str>) -> bool {
    label == "main" || task_id.is_some_and(|task| label == format!("review-{task}"))
}
async fn authorize_resource(label: &str, state: &DesktopState, id: &str) -> Result<(), String> {
    authorize(label)?;
    if label != "main" {
        let info = state.core.terminal_get(id).await?;
        if !allows_task(label, info.task_id.as_deref()) {
            return Err("backendText.m9ecc3357".into());
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn core_terminal_open(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    project_id: String,
    directory_index: usize,
    operation_id: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalInfo, String> {
    if window.label() != "main" {
        return Err("backendText.m4eb7494b".into());
    }
    let snapshot = state.core.bootstrap().await.map_err(|_| "backendText.m50891e4c")?;
    let project = snapshot
        .projects
        .iter()
        .find(|p| p.project_id.as_str() == project_id)
        .ok_or("backendText.projectMissing")?;
    let path = std::iter::once(&project.path)
        .chain(&project.additional_directories)
        .nth(directory_index)
        .ok_or("backendText.m3149b9e1")?;
    state
        .core
        .terminal_start(LaunchIntent {
            project_launch: None,
            operation_id,
            project_id,
            directory_index,
            task_id: None,
            command_id: None,
            label: "backendText.me2a76ef1".into(),
            cwd: path.into(),
            command: None,
            cols,
            rows,
        })
        .await
}

/// A shell on a connected host, outside any project.
#[tauri::command]
pub async fn core_terminal_open_on_target(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    target_id: String,
    operation_id: String,
    cols: u16,
    rows: u16,
) -> Result<TerminalInfo, String> {
    if window.label() != "main" {
        return Err("backendText.me8f2bd5d".into());
    }
    state
        .core
        .terminal_start_on_target(&target_id, &operation_id, cols, rows)
        .await
}

/// Ends a host's connection once its last shell closed, unless its projects need it.
#[tauri::command]
pub async fn core_terminal_release_target(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    target_id: String,
) -> Result<serde_json::Value, String> {
    if window.label() != "main" {
        return Err("backendText.m13337cd3".into());
    }
    state.core.terminal_release_target(&target_id).await
}

#[tauri::command]
pub async fn core_terminal_list(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    project_id: String,
) -> Result<Vec<TerminalInfo>, String> {
    authorize(window.label())?;
    Ok(state
        .core
        .terminal_list(&project_id)
        .await?
        .into_iter()
        .filter(|info| allows_task(window.label(), info.task_id.as_deref()))
        .collect())
}
#[tauri::command]
pub async fn core_terminal_get(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    id: String,
) -> Result<TerminalInfo, String> {
    authorize(window.label())?;
    let info = state.core.terminal_get(&id).await?;
    if !allows_task(window.label(), info.task_id.as_deref()) {
        return Err("backendText.m9ecc3357".into());
    }
    Ok(info)
}
#[tauri::command]
pub async fn core_terminal_read(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    id: String,
    after: u64,
) -> Result<OutputBatch, String> {
    authorize_resource(window.label(), &state, &id).await?;
    state.core.terminal_read(&id, after).await
}
#[tauri::command]
pub async fn core_terminal_input(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let id = request["terminalId"].as_str().ok_or("backendText.m833d6e18")?;
    authorize_resource(window.label(), &state, id).await?;
    state.core.terminal_input(window.label(), request).await
}
#[tauri::command]
pub async fn core_terminal_resize(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    authorize_resource(window.label(), &state, &id).await?;
    state.core.terminal_resize(&id, cols, rows).await
}
#[tauri::command]
pub async fn core_terminal_close(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    id: String,
    expected_epoch: String,
) -> Result<(), String> {
    authorize_resource(window.label(), &state, &id).await?;
    state
        .core
        .terminal_close(&id, &expected_epoch)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_host_adapter_preserves_review_task_scope() {
        assert!(allows_task("main", None));
        assert!(allows_task("review-task", Some("task")));
        assert!(!allows_task("review-task", None));
        assert!(!allows_task("review-task", Some("other")));
        assert!(!allows_task("other", Some("task")));
    }
}
