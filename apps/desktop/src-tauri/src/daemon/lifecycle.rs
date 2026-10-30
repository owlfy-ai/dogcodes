//! Core belongs to one desktop process. No detached update coordinator.
use super::{CoreClient, Endpoint};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

fn endpoint(directory: &Path) -> Option<Endpoint> {
    serde_json::from_slice(&std::fs::read(directory.join("service.json")).ok()?).ok()
}
async fn probe(directory: &Path, method: &str) -> Result<Value, String> {
    tokio::time::timeout(
        Duration::from_secs(2),
        CoreClient::Remote(directory.to_owned()).call(method, json!([])),
    )
    .await
    .map_err(|_| "background response timed out".to_owned())?
}

pub(crate) struct BackgroundService {
    directory: PathBuf,
    child: Mutex<Option<Child>>,
    operation: tokio::sync::Mutex<()>,
    closing: AtomicBool,
}
impl BackgroundService {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            child: Mutex::new(None),
            operation: tokio::sync::Mutex::new(()),
            closing: AtomicBool::new(false),
        }
    }
    pub async fn start(&self) -> Result<Arc<CoreClient>, String> {
        let _operation = self.operation.lock().await;
        self.launch().await
    }
    async fn launch(&self) -> Result<Arc<CoreClient>, String> {
        std::fs::create_dir_all(&self.directory).map_err(|_| "background directory unavailable")?;
        let pid = {
            let mut slot = self
                .child
                .lock()
                .map_err(|_| "background ownership unavailable")?;
            if self.closing.load(Ordering::Acquire) {
                return Err("desktop is closing".into());
            }
            let child =
                std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                    .arg("--core-child")
                    .arg(&self.directory)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|_| "background launch failed")?;
            let pid = child.id();
            *slot = Some(child);
            pid
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            if self.closing.load(Ordering::Acquire) {
                return Err("desktop is closing".into());
            }
            if let Some(e) = endpoint(&self.directory)
                && e.pid == pid
                && probe(&self.directory, "ping").await.is_ok()
            {
                return Ok(Arc::new(CoreClient::Remote(self.directory.clone())));
            }
            if self
                .child
                .lock()
                .map_err(|_| "background ownership unavailable")?
                .as_mut()
                .is_none_or(|child| child.try_wait().ok().flatten().is_some())
            {
                return Err("background exited during startup".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.stop_child();
        Err("background did not become ready".into())
    }
    fn stop_child(&self) {
        let mut slot = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(mut child) = slot.take() {
            stop_child(&mut child);
        }
    }
    pub fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        self.stop_child();
    }
    async fn busy(&self) -> bool {
        match endpoint(&self.directory) {
            Some(_) => probe(&self.directory, "is_idle").await != Ok(Value::Bool(true)),
            None => false,
        }
    }
}
impl Drop for BackgroundService {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn stop_child(child: &mut Child) {
    // EOF is a private lifetime signal. It also arrives if the UI crashes.
    drop(child.stdin.take());
    for _ in 0..150 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[tauri::command]
pub(crate) async fn core_service_restart(
    window: tauri::WebviewWindow,
    service: tauri::State<'_, Arc<BackgroundService>>,
    labels: CloseLabels,
    failure: String,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    let Ok(_operation) = service.operation.try_lock() else {
        return Ok(());
    };
    if !confirm(window.app_handle(), labels).await {
        return Ok(());
    }
    let owned = service.inner().clone();
    let result = tokio::task::spawn_blocking(move || owned.stop_child()).await;
    let result = match result {
        Ok(()) => service.launch().await.map(|_| ()),
        Err(_) => Err("background stop failed".into()),
    };
    if result.is_err() && !service.closing.load(Ordering::Acquire) {
        let _ = tokio::task::spawn_blocking(move || {
            window
                .app_handle()
                .dialog()
                .message(failure)
                .kind(MessageDialogKind::Error)
                .parent(&window)
                .blocking_show()
        })
        .await;
    }
    Ok(())
}

#[derive(Clone, Deserialize)]
pub(crate) struct CloseLabels {
    title: String,
    warning: String,
    confirm: String,
    cancel: String,
}
impl Default for CloseLabels {
    fn default() -> Self {
        Self { title: "Quit dog.codes?".into(), warning: "Closing dog.codes stops running tasks, terminals and phone connections. Minimize the window to keep them running.".into(), confirm: "Quit and stop background".into(), cancel: "Cancel".into() }
    }
}
#[derive(Default)]
pub(crate) struct ExitControl {
    pending: AtomicBool,
    pub approved: AtomicBool,
    labels: Mutex<CloseLabels>,
}
#[tauri::command]
pub(crate) fn desktop_close_labels(
    window: tauri::WebviewWindow,
    control: tauri::State<'_, ExitControl>,
    labels: CloseLabels,
) -> Result<(), String> {
    crate::require_main_artifact_window(window.label())?;
    *control
        .labels
        .lock()
        .map_err(|_| "close preferences unavailable")? = labels;
    Ok(())
}
async fn confirm(app: &tauri::AppHandle, labels: CloseLabels) -> bool {
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .message(labels.warning)
            .title(labels.title)
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom(
                labels.confirm,
                labels.cancel,
            ));
        if let Some(parent) = app.get_webview_window("main") {
            dialog = dialog.parent(&parent);
        }
        dialog.blocking_show()
    })
    .await
    .unwrap_or(false)
}

pub(crate) fn request_exit(app: &tauri::AppHandle) {
    crate::service_diagnostics::record("desktop_close_requested", "checking active work");
    let control = app.state::<ExitControl>();
    if control.pending.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let service = app
            .try_state::<Arc<BackgroundService>>()
            .map(|s| s.inner().clone());
        let busy = match service.as_ref() {
            Some(s) => s.busy().await,
            None => false,
        };
        crate::service_diagnostics::record(
            "desktop_close_check",
            if busy {
                "confirmation required"
            } else {
                "idle"
            },
        );
        if busy {
            let labels = app
                .state::<ExitControl>()
                .labels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if !confirm(&app, labels).await {
                app.state::<ExitControl>()
                    .pending
                    .store(false, Ordering::Release);
                return;
            }
        }
        if let Some(service) = service {
            let _ = tokio::task::spawn_blocking(move || service.shutdown()).await;
        }
        app.state::<ExitControl>()
            .approved
            .store(true, Ordering::Release);
        app.exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn desktop_shutdown_closes_the_private_parent_pipe_and_reaps_the_core() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("stopped");
        let child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("cat >/dev/null; printf stopped > \"$1\"")
            .arg("owned-core-fixture")
            .arg(&marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let service = BackgroundService::new(directory.path().into());
        *service.child.lock().unwrap() = Some(child);
        service.shutdown();
        service.shutdown();
        assert!(service.child.lock().unwrap().is_none());
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "stopped");
    }

    #[tokio::test]
    async fn closing_desktop_never_launches_a_replacement_core() {
        let directory = tempfile::tempdir().unwrap();
        let service = BackgroundService::new(directory.path().into());
        service.shutdown();
        assert!(service.launch().await.is_err());
        assert!(service.child.lock().unwrap().is_none());
        assert!(!directory.path().join("service.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn dropped_owner_stops_its_child() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("owner-dropped");
        let child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("cat >/dev/null; touch \"$1\"")
            .arg("owned-core-fixture")
            .arg(&marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let service = BackgroundService::new(directory.path().into());
        *service.child.lock().unwrap() = Some(child);
        drop(service);
        assert!(marker.exists());
    }
}
