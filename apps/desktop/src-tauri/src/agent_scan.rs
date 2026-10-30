//! Share only running probes. Completed results are never cached.
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
type Outcome = Option<Result<(), String>>;
#[derive(Default)]
pub(crate) struct ScanCoordinator {
    running: Arc<Mutex<HashMap<String, watch::Receiver<Outcome>>>>,
}
impl ScanCoordinator {
    /// An explicit rescan must not return a probe started before installation.
    /// Wait for that probe's side effects to settle, then start/share a fresh one.
    pub(crate) async fn run_fresh(
        &self,
        key: String,
        probe: impl Future<Output = Result<(), String>> + Send + 'static,
    ) -> Result<(), String> {
        let current = self
            .running
            .lock()
            .expect("scan registry")
            .get(&key)
            .cloned();
        if let Some(mut receiver) = current {
            loop {
                if receiver.borrow_and_update().is_some() {
                    break;
                }
                if receiver.changed().await.is_err() {
                    break;
                }
            }
        }
        self.run(key, probe).await
    }
    pub(crate) async fn run(
        &self,
        key: String,
        probe: impl Future<Output = Result<(), String>> + Send + 'static,
    ) -> Result<(), String> {
        let mut receiver = {
            let mut running = self.running.lock().expect("scan registry");
            if let Some(receiver) = running.get(&key) {
                receiver.clone()
            } else {
                let (sender, receiver) = watch::channel(None);
                running.insert(key.clone(), receiver.clone());
                let registry = self.running.clone();
                tokio::spawn(async move {
                    // A caller leaving the welcome page must not cancel another caller's scan.
                    let result = tokio::spawn(probe)
                        .await
                        .unwrap_or_else(|_| Err("agent detection failed".into()));
                    let mut running = registry.lock().expect("scan registry");
                    let _ = sender.send(Some(result));
                    running.remove(&key);
                });
                receiver
            }
        };
        loop {
            if let Some(result) = receiver.borrow_and_update().clone() {
                return result;
            }
            receiver
                .changed()
                .await
                .map_err(|_| "agent detection interrupted".to_string())?;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn explicit_rescan_waits_for_an_old_probe_then_returns_a_fresh_result() {
        let coordinator = Arc::new(ScanCoordinator::default());
        let (release, wait) = tokio::sync::oneshot::channel();
        let old = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move {
                coordinator
                    .run("kimi".into(), async {
                        wait.await.unwrap();
                        Err("not installed before the installer finished".into())
                    })
                    .await
            })
        };
        while coordinator.running.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        let fresh = coordinator.run_fresh("kimi".into(), async { Ok(()) });
        tokio::pin!(fresh);
        tokio::select! {
            biased;
            _ = &mut fresh => panic!("must not race the old scan's writes"),
            () = std::future::ready(()) => {}
        }
        release.send(()).unwrap();
        assert!(old.await.unwrap().is_err());
        assert_eq!(fresh.await, Ok(()));
    }
    #[tokio::test]
    async fn shares_running_probe_but_starts_new_probe_after_completion() {
        let coordinator = Arc::new(ScanCoordinator::default());
        let (release, wait) = tokio::sync::oneshot::channel();
        let first = {
            let c = coordinator.clone();
            tokio::spawn(async move {
                c.run("codex".into(), async {
                    wait.await.unwrap();
                    Ok(())
                })
                .await
            })
        };
        while coordinator.running.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
        let second = coordinator.run("codex".into(), async { panic!("duplicate probe") });
        tokio::pin!(second);
        tokio::select! {
            biased;
            _ = &mut second => panic!("probe must still be running"),
            () = std::future::ready(()) => {}
        }
        // A different provider completes even while Codex is blocked.
        coordinator
            .run("kimi".into(), async { Ok(()) })
            .await
            .unwrap();
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
        second.await.unwrap();
        assert_eq!(
            coordinator
                .run("codex".into(), async { Err("fresh probe".into()) })
                .await,
            Err("fresh probe".into())
        );
    }
}
