//! Window-owned forwarding only; dropping a bridge never stops a send.
use crate::{
    DesktopState, workspace_browser::bridges::WorkspaceWatchers,
    workspace_service::watch::WatchMessage,
};
#[tauri::command]
pub async fn core_submission_watch(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    watchers: tauri::State<'_, WorkspaceWatchers>,
    id: String,
    changes: tauri::ipc::Channel<WatchMessage>,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    uuid::Uuid::parse_str(&id).map_err(|_| "无效的状态订阅标识")?;
    let core = state.core.clone();
    let (ready, accepted) = tokio::sync::oneshot::channel();
    watchers.start(
        (window.label().into(), format!("submission:{id}")),
        async move {
            let mut stream = match core.submission_watch().await {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            if ready.send(Ok(())).is_err() {
                return;
            }
            loop {
                let message = stream
                    .next()
                    .await
                    .unwrap_or_else(|message| WatchMessage::Error {
                        code: "disconnected".into(),
                        message,
                    });
                let ended = matches!(message, WatchMessage::Error { .. });
                if changes.send(message).is_err() || ended {
                    return;
                }
            }
        },
    )?;
    accepted.await.map_err(|_| "状态订阅已取消")?
}
#[tauri::command]
pub fn core_submission_unwatch(
    window: tauri::WebviewWindow,
    watchers: tauri::State<'_, WorkspaceWatchers>,
    id: String,
) -> Result<(), String> {
    watchers.stop((window.label().into(), format!("submission:{id}")))
}
