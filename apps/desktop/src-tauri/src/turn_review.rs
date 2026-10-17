use crate::{DesktopState, review_window::require_task_window};
use agent_core::turn_review::{ReviewRequest, ReviewRun};
use agent_domain::{ExecutorRef, RunId, TaskId};

#[tauri::command]
pub async fn core_review_executors(
    state: tauri::State<'_, DesktopState>,
) -> Result<Vec<ExecutorRef>, String> {
    state.core.review_executors().await
}
#[tauri::command]
pub async fn core_turn_reviews(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: TaskId,
) -> Result<Vec<ReviewRun>, String> {
    require_task_window(window.label(), task_id.as_str())?;
    state.core.turn_reviews(&task_id).await
}
#[tauri::command]
pub async fn core_start_turn_review(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: ReviewRequest,
) -> Result<ReviewRun, String> {
    require_task_window(window.label(), request.task_id.as_str())?;
    state.core.start_turn_review(request).await
}
#[tauri::command]
pub async fn core_cancel_turn_review(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: TaskId,
    run_id: RunId,
) -> Result<(), String> {
    require_task_window(window.label(), task_id.as_str())?;
    state.core.cancel_turn_review(&task_id, &run_id).await
}
#[tauri::command]
pub async fn core_start_project_review(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: ReviewRequest,
) -> Result<ReviewRun, String> {
    require_task_window(window.label(), request.task_id.as_str())?;
    state.core.start_project_review(request).await
}

#[tauri::command]
pub async fn core_review_source(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: TaskId,
    run_id: RunId,
    file_id: String,
) -> Result<agent_core::review_analysis::ReviewSource, String> {
    require_task_window(window.label(), task_id.as_str())?;
    state.core.review_source(&task_id, &run_id, &file_id).await
}

#[tauri::command]
pub async fn core_fork_task(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    request: agent_core::task_fork::ForkRequest,
) -> Result<TaskId, String> {
    require_task_window(window.label(), request.task_id.as_str())?;
    state.core.fork_task(request).await
}

#[tauri::command]
pub async fn core_fork_receipt(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: TaskId,
    new_task_id: TaskId,
) -> Result<Option<agent_core::task_fork::ForkReceipt>, String> {
    require_task_window(window.label(), task_id.as_str())?;
    state.core.fork_receipt(task_id, new_task_id).await
}
