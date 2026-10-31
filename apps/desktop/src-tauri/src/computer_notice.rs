//! Non-activating desktop notice. Input waits for its painted acknowledgement.
use crate::DesktopState;
use agent_core::computer_use::{ActivityMode, ComputerActivity};
use serde_json::Value;
use tauri::Emitter;

#[tauri::command]
pub fn computer_use_permissions(
    window: tauri::WebviewWindow,
    prompt: bool,
) -> Result<Value, String> {
    if window.label() != "main" {
        return Err("main window required".into());
    }
    Ok(dogcodes_remote_desktop::computer_permissions::permissions(
        prompt,
    ))
}

fn is_notice(label: &str) -> bool {
    label
        .strip_prefix("computer-notice-")
        .is_some_and(|index| index.parse::<usize>().is_ok())
}

#[tauri::command]
pub async fn core_computer_notice(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    call: Value,
) -> Result<Value, String> {
    if !is_notice(window.label()) {
        return Err("computer notice window required".into());
    }
    state.core.computer_use_activity(call).await
}

pub fn start(
    app: &tauri::AppHandle,
    core: std::sync::Arc<crate::daemon::CoreClient>,
) -> tauri::Result<()> {
    let mut windows = vec![];
    for (index, monitor) in app.available_monitors()?.iter().enumerate() {
        let scale = monitor.scale_factor();
        // Use physical display bounds, including the very top edge above work-area insets.
        let size = monitor.size().to_logical::<f64>(scale);
        let origin = monitor.position().to_logical::<f64>(scale);
        let caption_top = crate::computer_notice_layout::caption_top(monitor);
        let window = tauri::WebviewWindowBuilder::new(
            app,
            format!("computer-notice-{index}"),
            tauri::WebviewUrl::App(
                format!("index.html?computerNotice=1&captionTop={caption_top}").into(),
            ),
        )
        .title("dog.codes · Computer use")
        .inner_size(size.width, 76.0 + caption_top)
        .position(origin.x, origin.y)
        .transparent(true)
        .background_color(tauri::utils::config::Color(0, 0, 0, 0))
        .shadow(false)
        .decorations(false)
        .resizable(false)
        .visible(false)
        .focused(false)
        .focusable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .build()?;
        // Borderless Windows WebViews can retain a small invisible non-client
        // inset. Align the rendered light itself with the display's pixel edge.
        #[cfg(windows)]
        if let (Ok(inner), Ok(outer)) = (window.inner_position(), window.outer_position()) {
            window.set_position(tauri::PhysicalPosition::new(
                monitor.position().x - (inner.x - outer.x),
                monitor.position().y - (inner.y - outer.y),
            ))?;
        }
        window.set_ignore_cursor_events(true)?;
        windows.push(window);
    }
    let (escape_sender, mut escape_events) = tokio::sync::mpsc::unbounded_channel();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut escape: Option<dogcodes_remote_desktop::escape::EscapeMonitor> = None;
        let mut escape_task: Option<agent_domain::TaskId> = None;
        loop {
            let Ok(mut activity) = core.computer_use_activity_stream().await else {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            };
            loop {
                let next = activity.borrow_and_update().clone();
                let Some(next) = next else {
                    // A lost service connection is not evidence that input stopped.
                    for window in &windows {
                        let _ = window.emit("computer-use-disconnected", ());
                    }
                    break;
                };
                if next.mode != ActivityMode::Idle {
                    let foreground = next.mode == ActivityMode::Foreground;
                    if foreground && escape.is_none() {
                        let sender = escape_sender.clone();
                        escape = dogcodes_remote_desktop::escape::EscapeMonitor::new(move || {
                            let _ = sender.send(());
                        })
                        .ok();
                    }
                    if foreground && escape.is_none() {
                        let _ = core.computer_use_activity(serde_json::json!({"type":"unavailable","operationId":next.operation_id})).await;
                        if activity.changed().await.is_err() {
                            break;
                        }
                        continue;
                    }
                    if let Some(escape) = &escape {
                        escape.arm(foreground);
                    }
                    escape_task = if foreground {
                        next.task_id.clone()
                    } else {
                        None
                    };
                }
                publish(&app, &windows, &next);
                let idle = next.mode == ActivityMode::Idle;
                tokio::select! {
                    biased;
                    event = escape_events.recv() => {
                        if event.is_some() && let Some(task) = escape_task.take()
                            && core.computer_use(task.clone(), serde_json::json!({"type":"set","enabled":false})).await.is_err() {
                                if let Some(escape) = &escape { escape.arm(true); }
                                escape_task = Some(task);
                                for window in &windows { let _ = window.emit("computer-use-disconnected", ()); }
                        }
                    }
                    changed = activity.changed() => { if changed.is_err() { break; } }
                    () = tokio::time::sleep(std::time::Duration::from_millis(900)), if idle => {
                        if let Some(escape) = &escape { escape.arm(false); }
                        escape_task = None;
                        for window in &windows { let _ = window.hide(); }
                        if activity.changed().await.is_err() { break; }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
    Ok(())
}

fn publish(app: &tauri::AppHandle, windows: &[tauri::WebviewWindow], next: &ComputerActivity) {
    let _ = app.emit_to("main", "computer-use-activity", next);
    for window in windows {
        if next.mode != ActivityMode::Idle && window.show().is_err() {
            continue;
        }
        let _ = window.emit("computer-use-activity", next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_notice_windows_may_acknowledge_input_warnings() {
        assert!(is_notice("computer-notice-0"));
        assert!(!is_notice("main"));
        assert!(!is_notice("computer-notice-"));
        assert!(!is_notice("computer-notice-other"));
    }
}
