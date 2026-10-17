use crate::{DesktopState, require_main_artifact_window};
use agent_domain::TaskId;
use tauri::{Emitter, Manager};

pub fn require_task_window(label: &str, task_id: &str) -> Result<(), String> {
    TaskId::parse(task_id).map_err(|_| "invalid task")?;
    if label == "main" || label == format!("review-{task_id}") {
        Ok(())
    } else {
        Err("此窗口不能访问其他任务的 Diff".into())
    }
}

#[tauri::command]
pub async fn core_review_window_open(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
    view: Option<serde_json::Value>,
) -> Result<(), String> {
    require_main_artifact_window(window.label())?;
    require_task_window("main", &task_id)?;
    let task = state
        .core
        .bootstrap()
        .await?
        .tasks
        .into_iter()
        .find(|t| t.task_id.to_string() == task_id)
        .ok_or("任务不存在")?;
    let label = format!("review-{task_id}");
    if let Some(existing) = window.app_handle().get_webview_window(&label) {
        existing.unminimize().map_err(|e| e.to_string())?;
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let detached = tauri::WebviewWindowBuilder::new(
        window.app_handle(),
        &label,
        tauri::WebviewUrl::App(format!("index.html?reviewTask={task_id}").into()),
    )
    .initialization_script(format!(
        "window.__REVIEW_VIEW__ = {};",
        view.unwrap_or(serde_json::Value::Null)
    ))
    .title(format!("Diff · {}", task.title))
    .inner_size(1120.0, 820.0)
    .min_inner_size(620.0, 480.0)
    .resizable(true)
    .build()
    .map_err(|e| e.to_string())?;
    let app = window.app_handle().clone();
    let closed_task = task_id.clone();
    detached.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let _ = app.emit_to("main", "review-window-closed", &closed_task);
        }
    });
    detached.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn core_review_window_is_open(
    window: tauri::WebviewWindow,
    task_id: String,
) -> Result<bool, String> {
    require_main_artifact_window(window.label())?;
    require_task_window("main", &task_id)?;
    Ok(window
        .app_handle()
        .get_webview_window(&format!("review-{task_id}"))
        .is_some())
}

#[tauri::command]
pub fn core_review_window_close(
    window: tauri::WebviewWindow,
    task_id: String,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    if window.label() == "main" {
        if let Some(detached) = window
            .app_handle()
            .get_webview_window(&format!("review-{task_id}"))
        {
            detached.close().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    window.close().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn core_review_window_running(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    task_id: String,
) -> Result<bool, String> {
    require_task_window(window.label(), &task_id)?;
    let snapshot = state.core.bootstrap().await?;
    let task = snapshot
        .tasks
        .iter()
        .find(|t| t.task_id.to_string() == task_id)
        .ok_or("任务不存在")?;
    Ok(task.active_primary_run.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_window_is_scoped_to_its_task() {
        let task = TaskId::new().to_string();
        assert!(require_task_window("main", &task).is_ok());
        assert!(require_task_window(&format!("review-{task}"), &task).is_ok());
        assert!(require_task_window(&format!("review-{}", TaskId::new()), &task).is_err());
        assert!(require_task_window("companion", &task).is_err());
        assert!(require_task_window("main", "invalid").is_err());
    }
}
