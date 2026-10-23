//! Service-owned idle-sleep policy. Never inhibits locking or explicit sleep.
use super::{Arc, Core, Duration, PathBuf, ServiceControl, Value, failure, json};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};
pub(super) const METHODS: &[&str] = &["power_preferences", "power_preferences_set"];

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn setting_is_persistent_and_resending_it_is_harmless() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("power.json");
        let host = Arc::new(Host::new(path.clone()));
        assert_eq!(
            host.call("power_preferences", Value::Null).await.unwrap()["enabled"],
            true
        );
        let off = json!({"enabled": false});
        let first = host
            .call("power_preferences_set", off.clone())
            .await
            .unwrap();
        assert_eq!(first["enabled"], false);
        // A resend after a lost reply sets the same value again: no harm.
        assert_eq!(
            host.call("power_preferences_set", off).await.unwrap(),
            first
        );
        let reopened = Arc::new(Host::new(path));
        assert_eq!(
            reopened
                .call("power_preferences", Value::Null)
                .await
                .unwrap()["enabled"],
            false
        );
        assert!(
            reopened
                .call("power_preferences_set", json!({"enabled": "no"}))
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn a_damaged_file_is_reported_and_reread_after_repair() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("power.json");
        std::fs::write(&path, "not json").unwrap();
        let host = Arc::new(Host::new(path.clone()));
        assert!(host.call("power_preferences", Value::Null).await.is_err());
        std::fs::write(&path, r#"{"enabled":false}"#).unwrap();
        assert_eq!(
            host.call("power_preferences", Value::Null).await.unwrap()["enabled"],
            false
        );
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Set {
    enabled: bool,
}
/// Older files also carried a revision and the last request; both are ignored.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Policy {
    enabled: bool,
}
impl Default for Policy {
    fn default() -> Self {
        Self { enabled: true }
    }
}
pub(super) struct Host {
    path: PathBuf,
    policy: StdMutex<Option<Result<Policy, String>>>,
    started: AtomicBool,
    pub(super) desktop: AtomicBool,
    status: StdMutex<(bool, Option<String>)>,
}
impl Host {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            policy: StdMutex::new(None),
            started: AtomicBool::new(false),
            desktop: AtomicBool::new(false),
            status: StdMutex::new((false, None)),
        }
    }
    fn read_policy(&self, cache: &mut Option<Result<Policy, String>>) -> Result<Policy, String> {
        if let Some(Ok(value)) = cache.as_ref() {
            return Ok(value.clone());
        }
        let value = match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice::<Policy>(&bytes)
                .map_err(|_| "电源偏好损坏，未覆盖原设置".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::default()),
            Err(_) => Err("电源偏好不可读取".into()),
        };
        *cache = Some(value.clone());
        value
    }
    fn loaded(&self) -> Result<Policy, String> {
        let mut cache = self.policy.lock().map_err(|_| "电源偏好不可用")?;
        self.read_policy(&mut cache)
    }
    fn persist(
        &self,
        cache: &mut Option<Result<Policy, String>>,
        policy: &Policy,
    ) -> Result<(), String> {
        // Any ambiguous failure forces a fresh disk read next time.
        *cache = None;
        let directory = self.path.parent().ok_or("电源偏好路径无效")?;
        std::fs::create_dir_all(directory).map_err(|_| "电源偏好目录不可写入")?;
        let mut file =
            tempfile::NamedTempFile::new_in(directory).map_err(|_| "电源偏好不可写入")?;
        serde_json::to_writer(&mut file, policy).map_err(|_| "电源偏好不可写入")?;
        file.flush()
            .and_then(|()| file.as_file_mut().sync_all())
            .map_err(|_| "电源偏好保存结果待核实")?;
        file.persist(&self.path)
            .map_err(|_| "电源偏好保存结果待核实")?;
        #[cfg(unix)]
        std::fs::File::open(directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| "电源偏好保存结果待核实，请重新读取")?;
        *cache = Some(Ok(policy.clone()));
        Ok(())
    }
    fn snapshot(&self, policy: &Policy) -> Result<Value, String> {
        let status = self.status.lock().map_err(|_| "电源状态不可用")?;
        Ok(json!({"enabled":policy.enabled,"active":status.0,"error":status.1}))
    }
    pub(super) async fn call(self: &Arc<Self>, method: &str, args: Value) -> Result<Value, String> {
        let host = self.clone();
        let method = method.to_string();
        tokio::task::spawn_blocking(move || {
            let mut cache = host.policy.lock().map_err(|_| "电源偏好不可用")?;
            let mut policy = host.read_policy(&mut cache)?;
            match method.as_str() {
                "power_preferences" => host.snapshot(&policy),
                "power_preferences_set" => {
                    let request: Set = serde_json::from_value(args).map_err(failure)?;
                    if policy.enabled != request.enabled {
                        policy.enabled = request.enabled;
                        host.persist(&mut cache, &policy)?;
                    }
                    host.snapshot(&policy)
                }
                _ => Err("unknown power operation".into()),
            }
        })
        .await
        .map_err(failure)?
    }
    pub(super) fn start(
        self: &Arc<Self>,
        core: &Arc<Core>,
        control: std::sync::Weak<ServiceControl>,
    ) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
        let host = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("idle-sleep-policy".into())
            .spawn(move || {
                let mut awake = None;
                let mut busy = false;
                let mut last_attempt = None;
                while let Some(host) = host.upgrade() {
                    let enabled = host.loaded().is_ok_and(|p| p.enabled);
                    let wanted = enabled && (busy || host.desktop.load(Ordering::Acquire));
                    if !wanted {
                        awake = None;
                        if let Ok(mut status) = host.status.lock() {
                            *status = (false, None);
                        }
                    } else if awake.is_none()
                        && last_attempt.is_none_or(|last: std::time::Instant| {
                            last.elapsed() > Duration::from_secs(10)
                        })
                    {
                        last_attempt = Some(std::time::Instant::now());
                        match dogcodes_remote_desktop::keep_awake() {
                            Ok(guard) => {
                                awake = Some(guard);
                                if let Ok(mut status) = host.status.lock() {
                                    *status = (true, None);
                                }
                            }
                            Err(message) => {
                                if let Ok(mut status) = host.status.lock() {
                                    *status = (false, Some(message));
                                }
                            }
                        }
                    }
                    drop(host);
                    match rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(active) => busy = active,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                }
            });
        if spawned.is_err() {
            if let Ok(mut status) = self.status.lock() {
                *status = (false, Some("无法启动电源管理线程".into()));
            }
            return;
        }
        let core = Arc::downgrade(core);
        tokio::spawn(async move {
            loop {
                let (Some(core), Some(control)) = (core.upgrade(), control.upgrade()) else {
                    return;
                };
                let active = control.terminals.has_active()
                    || control.project_runs.has_active()
                    || core.has_active_reviews().await
                    || core
                        .bootstrap()
                        .await
                        .map_or(true, |s| !s.active_runs.is_empty());
                match tx.try_send(active) {
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => return,
                    Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                }
                drop(control);
                drop(core);
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }
}
