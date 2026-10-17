use crate::{DesktopState, review_window::require_task_window};
use agent_domain::TaskId;

#[tauri::command]
pub async fn core_task_deletion(
    window: tauri::Window,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) -> Result<Option<agent_core::recovery::TaskDeletion>, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .task_deletion(TaskId::parse(task_id).map_err(|e| e.to_string())?)
        .await
}
