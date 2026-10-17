//! A window owns only cancellable forwarding tasks. The OS watcher lives in the
//! service, and dropping the stream releases it even without an unwatch RPC.
use crate::{
    DesktopState,
    review_window::require_task_window,
    workspace_service::{DirectoryRequest, watch::WatchMessage},
};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};
type Key = (String, String);
struct Bridge {
    epoch: uuid::Uuid,
    abort: tokio::task::AbortHandle,
}
#[derive(Clone, Default)]
pub struct WorkspaceWatchers(Arc<Mutex<HashMap<Key, Bridge>>>);
struct Cleanup {
    registry: WorkspaceWatchers,
    key: Key,
    epoch: uuid::Uuid,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.registry.0.lock()
            && entries
                .get(&self.key)
                .is_some_and(|entry| entry.epoch == self.epoch)
        {
            entries.remove(&self.key);
        }
    }
}
impl WorkspaceWatchers {
    pub fn close_window(&self, label: &str) {
        if let Ok(mut entries) = self.0.lock() {
            entries.retain(|(window, _), entry| {
                if window == label {
                    entry.abort.abort();
                    false
                } else {
                    true
                }
            });
        }
    }
    pub(crate) fn stop(&self, key: Key) -> Result<(), String> {
        if let Some(entry) = self.0.lock().map_err(|_| "监听状态不可用")?.remove(&key) {
            entry.abort.abort();
        }
        Ok(())
    }
    pub(crate) fn start(
        &self,
        key: Key,
        work: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), String> {
        let epoch = uuid::Uuid::new_v4();
        let cleanup = Cleanup {
            registry: self.clone(),
            key: key.clone(),
            epoch,
        };
        let (start, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _cleanup = cleanup;
            if ready.await.is_ok() {
                work.await;
            }
        });
        // Gate the task so a quick failure cannot clean up before registration.
        let mut entries = self.0.lock().map_err(|_| "监听状态不可用")?;
        if let Some(old) = entries.insert(
            key,
            Bridge {
                epoch,
                abort: task.abort_handle(),
            },
        ) {
            old.abort.abort();
        }
        let _ = start.send(());
        Ok(())
    }
}
#[tauri::command]
#[expect(
    clippy::too_many_arguments,
    reason = "Tauri injects window and state alongside named IPC arguments"
)]
pub async fn core_workspace_watch(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, DesktopState>,
    watchers: tauri::State<'_, WorkspaceWatchers>,
    task_id: String,
    root: usize,
    path: String,
    id: String,
    changes: tauri::ipc::Channel<WatchMessage>,
) -> Result<(), String> {
    require_task_window(window.label(), &task_id)?;
    uuid::Uuid::parse_str(&id).map_err(|_| "无效的目录订阅标识")?;
    let core = state.core.clone();
    let (ready, accepted) = tokio::sync::oneshot::channel();
    watchers.start((window.label().into(), id), async move {
        let mut stream = match core
            .workspace_watch(DirectoryRequest {
                task_id,
                root,
                path,
            })
            .await
        {
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
    })?;
    accepted.await.map_err(|_| "目录订阅已取消")?
}
#[tauri::command]
pub fn core_workspace_unwatch(
    window: tauri::WebviewWindow,
    watchers: tauri::State<'_, WorkspaceWatchers>,
    id: String,
) -> Result<(), String> {
    watchers.stop((window.label().into(), id))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn workspace_bridge_cancellation_during_startup_and_old_cleanup_are_isolated() {
        let registry = WorkspaceWatchers::default();
        let key = ("main".into(), "same-id".into());
        registry.start(key.clone(), std::future::pending()).unwrap();
        let first_epoch = registry.0.lock().unwrap()[&key].epoch;
        registry.start(key.clone(), std::future::pending()).unwrap();
        let second_epoch = registry.0.lock().unwrap()[&key].epoch;
        assert_ne!(first_epoch, second_epoch);
        tokio::task::yield_now().await;
        assert_eq!(registry.0.lock().unwrap()[&key].epoch, second_epoch);
        registry
            .start(
                ("review-other".into(), "other".into()),
                std::future::pending(),
            )
            .unwrap();
        registry.close_window("main");
        tokio::task::yield_now().await;
        assert_eq!(registry.0.lock().unwrap().len(), 1);
        registry
            .stop(("review-other".into(), "other".into()))
            .unwrap();
        tokio::task::yield_now().await;
        assert!(registry.0.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn workspace_bridge_failure_removes_itself_without_waiting_for_window_close() {
        let registry = WorkspaceWatchers::default();
        registry
            .start(("main".into(), "id".into()), async {})
            .unwrap();
        tokio::task::yield_now().await;
        assert!(registry.0.lock().unwrap().is_empty());
    }
}
