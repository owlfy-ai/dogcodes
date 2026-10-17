//! Bounded invalidations, not a second business-state store. Reconnect always queries facts.
use crate::workspace_service::watch::WatchMessage;
use crate::{
    DesktopState, review_window::require_task_window, workspace_browser::bridges::WorkspaceWatchers,
};
use agent_core::Core;
use std::sync::Arc;

pub(crate) struct Changes(Arc<crate::state_changes::Changes>);
impl Changes {
    pub fn new() -> Self {
        Self(Arc::new(crate::state_changes::Changes::new()))
    }
    pub fn source(&self) -> Arc<crate::state_changes::Changes> {
        self.0.clone()
    }
    pub fn changed(&self) {
        self.0.changed();
    }
    pub async fn open(&self, core: Arc<Core>, task: String) -> Result<Subscription, String> {
        let changes = self.0.subscribe()?;
        super::service::require_task(&core, &task).await?;
        Ok(Subscription {
            core,
            task,
            changes,
        })
    }
}
pub(crate) struct Subscription {
    core: Arc<Core>,
    task: String,
    changes: crate::state_changes::Subscription,
}
impl Subscription {
    pub async fn next(&mut self) -> Result<WatchMessage, String> {
        let message = self.changes.next().await?;
        super::service::require_task(&self.core, &self.task).await?;
        Ok(message)
    }
}

#[tauri::command]
pub async fn core_review_watch(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    watchers: tauri::State<'_, WorkspaceWatchers>,
    task_id: String,
    id: String,
    changes: tauri::ipc::Channel<WatchMessage>,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    uuid::Uuid::parse_str(&id).map_err(|_| "无效的状态订阅标识")?;
    let core = state.core.clone();
    let (ready, accepted) = tokio::sync::oneshot::channel();
    watchers.start(
        (window.label().into(), format!("review:{id}")),
        async move {
            let mut stream = match core.review_watch(task_id).await {
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
pub fn core_review_unwatch(
    window: tauri::WebviewWindow,
    watchers: tauri::State<'_, WorkspaceWatchers>,
    id: String,
) -> Result<(), String> {
    watchers.stop((window.label().into(), format!("review:{id}")))
}
