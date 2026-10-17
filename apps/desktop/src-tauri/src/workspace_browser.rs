//! Desktop access checks and adapters for service-owned workspace operations.
use crate::workspace_service::DirectoryRequest;
pub(crate) use crate::workspace_service::preview_bytes;
pub use crate::workspace_service::{Entry, FileMatches, Preview};
use crate::{DesktopState, review_window::require_task_window};
use std::path::PathBuf;
pub mod bridges;
pub use bridges::WorkspaceWatchers;

#[tauri::command]
pub async fn core_workspace_roots(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) -> Result<Vec<PathBuf>, String> {
    require_task_window(window.label(), &task_id)?;
    state.core.workspace_roots(task_id).await
}
#[tauri::command]
pub async fn core_workspace_directory(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    root: usize,
    path: String,
) -> Result<Vec<Entry>, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .workspace_directory(DirectoryRequest {
            task_id,
            root,
            path,
        })
        .await
}
#[tauri::command]
pub async fn core_workspace_preview(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    root: usize,
    path: String,
) -> Result<Preview, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .workspace_preview(DirectoryRequest {
            task_id,
            root,
            path,
        })
        .await
}
#[tauri::command]
pub async fn core_workspace_find_files(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    query: String,
    hide_noise: Option<bool>,
) -> Result<FileMatches, String> {
    require_task_window(window.label(), &task_id)?;
    state
        .core
        .workspace_find_files(task_id, query, hide_noise.unwrap_or(false))
        .await
}
