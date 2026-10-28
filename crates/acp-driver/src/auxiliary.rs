//! Read-only native calls retain their original resident until a response or a
//! proven native close. Caller deadlines do not turn them into new-session work.
use super::{ConnectionHandle, DriverError, Shared, sessions};
use std::{
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
#[derive(Clone)]
pub(super) struct Call(Arc<Lease>);
struct Lease {
    owner: Weak<Shared>,
    resident: Arc<sessions::Resident>,
    pending: Arc<AtomicUsize>,
    session: String,
    turn: Option<Arc<super::turn::Turn>>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.resident.auxiliary.store(false, Ordering::Release);
        self.pending.fetch_sub(1, Ordering::AcqRel);
    }
}
impl Call {
    /// Caller holds lifecycle; the command and its consumer share this lease.
    pub fn acquire(
        shared: &Arc<Shared>,
        id: &str,
        connection: Option<&ConnectionHandle>,
    ) -> Option<Self> {
        if shared.pending_auxiliary.load(Ordering::Acquire) >= 8 {
            return None;
        }
        let resident = shared
            .sessions
            .lock()
            .expect("ACP sessions")
            .get(id)?
            .clone();
        if resident.is_closing()
            || resident.epoch.load(Ordering::Acquire)
            || connection
                .is_some_and(|c| c.unavailable() || !Arc::ptr_eq(&c.finished, &resident.epoch))
            || resident
                .auxiliary
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return None;
        }
        shared.pending_auxiliary.fetch_add(1, Ordering::AcqRel);
        let turn = resident.turn.lock().expect("ACP session turn").clone();
        Some(Self(Arc::new(Lease {
            owner: Arc::downgrade(shared),
            resident,
            pending: shared.pending_auxiliary.clone(),
            session: id.into(),
            turn,
        })))
    }
    pub fn session(&self) -> &str {
        &self.0.session
    }
    pub fn with_current<T>(&self, apply: impl FnOnce(&Shared) -> T) -> Option<T> {
        let owner = self.0.owner.upgrade()?;
        let _gate = owner.lifecycle.lock().expect("ACP lifecycle");
        if owner.setup_in_progress.load(Ordering::Acquire)
            || !same_turn(
                self.0.turn.as_ref(),
                self.0
                    .resident
                    .turn
                    .lock()
                    .expect("ACP session turn")
                    .as_ref(),
            )
            || self.0.resident.is_closing()
            || self.0.resident.epoch.load(Ordering::Acquire)
            || !owner
                .sessions
                .lock()
                .expect("ACP sessions")
                .get(self.session())
                .is_some_and(|r| Arc::ptr_eq(r, &self.0.resident))
        {
            return None;
        }
        Some(apply(&owner))
    }
    pub async fn wait<T>(
        &self,
        read: impl std::future::Future<Output = Result<T, DriverError>>,
    ) -> Result<T, DriverError> {
        tokio::pin!(read);
        loop {
            if self.0.resident.native_closed.load(Ordering::Acquire)
                || self.0.resident.epoch.load(Ordering::Acquire)
            {
                return Err(super::driver_error_from_channel());
            }
            tokio::select! {
                biased;
                result=&mut read=>return result,
                ()=tokio::time::sleep(Duration::from_millis(20))=>{},
            }
        }
    }
}

/// File reads can serve historical usage without starting an Agent. They still
/// must not publish into a newly restored resident or a different workspace.
pub(super) struct Snapshot {
    session: String,
    resident: Option<Arc<sessions::Resident>>,
    pub directory: Option<std::path::PathBuf>,
    turn: Option<Arc<super::turn::Turn>>,
}
impl Snapshot {
    pub fn capture(shared: &Shared, id: &str) -> Self {
        let _gate = shared.lifecycle.lock().expect("ACP lifecycle");
        let resident = shared
            .sessions
            .lock()
            .expect("ACP sessions")
            .get(id)
            .cloned();
        let turn = resident
            .as_ref()
            .and_then(|r| r.turn.lock().expect("ACP session turn").clone());
        Self {
            session: id.into(),
            resident,
            turn,
            directory: shared
                .quota
                .directories
                .lock()
                .expect("usage directories")
                .get(id)
                .cloned(),
        }
    }
    pub fn apply<T>(&self, shared: &Shared, read: impl FnOnce() -> T) -> Option<T> {
        let _gate = shared.lifecycle.lock().expect("ACP lifecycle");
        if shared.setup_in_progress.load(Ordering::Acquire) {
            return None;
        }
        let current = shared
            .sessions
            .lock()
            .expect("ACP sessions")
            .get(&self.session)
            .cloned();
        let same = match (&self.resident, &current) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let current_turn = current
            .as_ref()
            .and_then(|r| r.turn.lock().expect("ACP session turn").clone());
        if !same
            || !same_turn(self.turn.as_ref(), current_turn.as_ref())
            || current
                .as_ref()
                .is_some_and(|r| r.is_closing() || r.epoch.load(Ordering::Acquire))
            || shared
                .quota
                .directories
                .lock()
                .expect("usage directories")
                .get(&self.session)
                != self.directory.as_ref()
        {
            return None;
        }
        Some(read())
    }
}
fn same_turn(a: Option<&Arc<super::turn::Turn>>, b: Option<&Arc<super::turn::Turn>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
impl super::AcpDriver {
    pub(super) async fn refresh_kiro_context(&self, id: &str) {
        let admitted = {
            let connection = self.shared.connection.lock().await;
            let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
            if self.shared.turn_active.load(Ordering::Acquire)
                || self.shared.setup_in_progress.load(Ordering::Acquire)
            {
                return;
            }
            let Some(connection) = connection.as_ref() else {
                return;
            };
            Call::acquire(&self.shared, id, Some(connection))
                .map(|scope| (connection.clone(), scope))
        };
        let Some((connection, scope)) = admitted else {
            return;
        };
        let (reply, response) = tokio::sync::oneshot::channel();
        if connection
            .commands
            .send(super::Command::ReadKiroContext {
                scope: scope.clone(),
                reply,
            })
            .is_err()
        {
            return;
        }
        if let Ok(Ok(Ok(value))) = tokio::time::timeout(Duration::from_secs(2), response).await {
            scope.with_current(|shared| shared.quota.observe_kiro_context(id, &value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn historical_file_snapshot_is_readable_but_never_publishes_into_a_changed_directory() {
        let driver = super::super::AcpDriver::new(
            "kiro",
            &agent_domain::InstallationId::new().to_string(),
            super::super::AcpLaunch {
                executable: "/missing-must-not-spawn".into(),
                args: vec![],
            },
            None,
            super::super::SessionResumeCapability::Supported,
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        driver
            .shared
            .quota
            .directories
            .lock()
            .unwrap()
            .insert("s".into(), root.path().join("original"));
        let snapshot = Snapshot::capture(&driver.shared, "s");
        assert_eq!(snapshot.apply(&driver.shared, || 42), Some(42));
        driver
            .shared
            .quota
            .directories
            .lock()
            .unwrap()
            .insert("s".into(), root.path().join("replacement"));
        assert!(
            snapshot
                .apply(&driver.shared, || panic!(
                    "stale file usage overwrote another workspace"
                ))
                .is_none()
        );
    }
}
