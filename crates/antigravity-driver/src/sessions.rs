use super::{AntigravityDriver, DriverError, Ordering, SessionHandle, SessionProcess};
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct TurnLease {
    pub handle: SessionHandle,
    pub turn: uuid::Uuid,
}

pub(super) fn stale_scope() -> DriverError {
    DriverError::SessionIncompatible("Antigravity session handle is no longer current".into())
}

#[derive(Clone)]
pub(super) struct Retired {
    pub handle: SessionHandle,
    pub native: Option<String>,
}

impl SessionProcess {
    pub fn mark_started(&self) {
        let _gate = self.lifecycle.lock().expect("session lifecycle");
        if !self.turn_active.load(Ordering::Acquire) || *self.closing.borrow() {
            return;
        }
        let Some(turn) = *self.turn.lock().expect("turn scope") else {
            return;
        };
        let mut started = self.started.lock().expect("started turn");
        if *started == Some(turn) {
            return;
        }
        if let Some(sink) = self.updates.lock().expect("updates sink").as_ref() {
            let _ = sink.send(super::DriverEvent::TurnStarted {
                native_turn_id: None,
            });
            *started = Some(turn);
        }
    }

    pub fn observe_native(&self, id: &str) -> Result<bool, DriverError> {
        let _gate = self.lifecycle.lock().expect("session lifecycle");
        let mut native = self.native_id.lock().expect("native id");
        if id.is_empty() || id.len() > 256 || native.as_ref().is_some_and(|old| old != id) {
            self.input_failed.store(true, Ordering::Release);
            self.closing.send_replace(true);
            return Err(DriverError::SessionIncompatible(
                "Antigravity reported a different conversation".into(),
            ));
        }
        let bound = native.is_none();
        *native = Some(id.into());
        Ok(bound)
    }
    pub fn handle(&self) -> SessionHandle {
        self.scope.lock().expect("session scope").clone()
    }

    pub fn validate_scope(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        if !self.handle().same_scope(handle) {
            return Err(stale_scope());
        }
        Ok(())
    }

    pub fn lease(&self) -> Option<TurnLease> {
        let _gate = self.lifecycle.lock().expect("session lifecycle");
        if *self.closing.borrow() || !self.turn_active.load(Ordering::Acquire) {
            return None;
        }
        Some(TurnLease {
            handle: self.handle(),
            turn: (*self.turn.lock().expect("turn scope"))?,
        })
    }

    /// Call under lifecycle when admitting/answering a Hook. The original
    /// socket keeps this lease while waiting for payload and user input.
    pub fn owns(&self, lease: &TurnLease) -> bool {
        self.handle().same_scope(&lease.handle)
            && *self.turn.lock().expect("turn scope") == Some(lease.turn)
            && self.turn_active.load(Ordering::Acquire)
            && !self.interrupt_requested.load(Ordering::Acquire)
            && !*self.closing.borrow()
    }

    pub fn request_close(
        &self,
        handle: &SessionHandle,
        only_idle: bool,
        interrupted: bool,
    ) -> Result<(), DriverError> {
        let _gate = self.lifecycle.lock().expect("session lifecycle");
        self.validate_scope(handle)?;
        if only_idle && self.turn_active.load(Ordering::Acquire) {
            return Err(DriverError::CapabilityUnavailable(
                "Antigravity 已有运行中的会话".into(),
            ));
        }
        if interrupted {
            self.interrupt_requested.store(true, Ordering::Release);
        }
        self.stop_requested.store(true, Ordering::Release);
        self.closing.send_replace(true);
        self.pending_approvals.lock().expect("approvals").clear();
        self.pending_questions.lock().expect("questions").clear();
        Ok(())
    }

    pub async fn confirmed(&self) {
        let mut cleanup = self.cleanup.clone();
        loop {
            if matches!(*cleanup.borrow_and_update(), Some(Ok(()))) {
                return;
            }
            if cleanup.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    pub fn observe_owner(self: &Arc<Self>, mut owner: tokio::sync::watch::Receiver<bool>) {
        let weak = Arc::downgrade(self);
        let mut cleanup = self.cleanup.clone();
        tokio::spawn(async move {
            loop {
                if *owner.borrow_and_update() {
                    if let Some(process) = weak.upgrade() {
                        let handle = process.handle();
                        let _ = process.request_close(&handle, false, true);
                    }
                    return;
                }
                if matches!(*cleanup.borrow_and_update(), Some(Ok(()))) {
                    return;
                }
                tokio::select! {
                    change = owner.changed() => if change.is_err() {
                        if let Some(process) = weak.upgrade() {
                            let handle = process.handle();
                            let _ = process.request_close(&handle, false, true);
                        }
                        return;
                    },
                    change = cleanup.changed() => if change.is_err() { return; },
                }
            }
        });
    }
}

/// Before the handle is returned, caller abandonment still closes the exact
/// installed candidate. Its supervisor retains OS scope and auth exclusion.
pub(super) struct Startup {
    pub process: Arc<SessionProcess>,
    pub claimed: bool,
}
impl Drop for Startup {
    fn drop(&mut self) {
        if !self.claimed {
            let _ = self
                .process
                .request_close(&self.process.handle(), false, true);
        }
    }
}

impl AntigravityDriver {
    pub(super) async fn find(&self, id: &str) -> Option<(String, Arc<SessionProcess>)> {
        self.processes
            .lock()
            .await
            .iter()
            .find(|(key, process)| {
                key.as_str() == id
                    || process.native_id.lock().expect("native id").as_deref() == Some(id)
            })
            .map(|(key, process)| (key.clone(), process.clone()))
    }

    pub(super) async fn process_for(
        &self,
        handle: &SessionHandle,
    ) -> Result<Arc<SessionProcess>, DriverError> {
        let (_, process) = self.find(&handle.native_session_id).await.ok_or_else(|| {
            DriverError::SessionMissing("Antigravity session is not resident".into())
        })?;
        process.validate_scope(handle)?;
        Ok(process)
    }

    pub(super) async fn remove_retired(
        &self,
        key: &str,
        process: &Arc<SessionProcess>,
        handle: &SessionHandle,
    ) {
        let mut processes = self.processes.lock().await;
        if processes.get(key).is_some_and(|p| Arc::ptr_eq(p, process)) {
            processes.remove(key);
            let mut retired = self.retired.lock().expect("retired scopes");
            let native = process.native_id.lock().expect("native id").clone();
            let record = Retired {
                handle: handle.clone(),
                native: native.clone(),
            };
            retired.insert(handle.native_session_id.clone(), record.clone());
            if let Some(native) = native {
                retired.insert(native, record);
            }
        }
    }
}
