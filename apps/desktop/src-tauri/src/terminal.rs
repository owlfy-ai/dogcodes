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
        Err("此窗口不能访问终端".into())
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
            return Err("此窗口不能访问其他会话的终端".into());
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
        return Err("请从主窗口创建项目终端".into());
    }
    let snapshot = state.core.bootstrap().await.map_err(|_| "项目目录不可用")?;
    let project = snapshot
        .projects
        .iter()
        .find(|p| p.project_id.as_str() == project_id)
        .ok_or("项目不存在")?;
    let path = std::iter::once(&project.path)
        .chain(&project.additional_directories)
        .nth(directory_index)
        .ok_or("项目目录不存在")?;
    state
        .core
        .terminal_start(LaunchIntent {
            project_launch: None,
            operation_id,
            project_id,
            directory_index,
            task_id: None,
            command_id: None,
            label: "终端".into(),
            cwd: path.into(),
            command: None,
            cols,
            rows,
        })
        .await
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
        return Err("此窗口不能访问其他会话的终端".into());
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
    let id = request["terminalId"].as_str().ok_or("缺少终端标识")?;
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
) -> Result<(), String> {
    authorize_resource(window.label(), &state, &id).await?;
    state.core.terminal_stop(&id).await.map(|_| ())
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
