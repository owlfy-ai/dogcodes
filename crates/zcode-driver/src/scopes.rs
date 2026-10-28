//! Runtime authority is separate from `ZCode`'s persisted native session ID.
use super::{Connection, DriverError, SessionHandle, SessionState, ZcodeDriver};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
pub(super) type Retired = Arc<Mutex<HashMap<String, SessionHandle>>>;
pub(super) fn stale() -> DriverError {
    DriverError::SessionIncompatible("ZCode session handle is no longer current".into())
}
impl SessionState {
    pub fn publish(
        &self,
        handle: &SessionHandle,
        id: String,
        pending: super::PendingApproval,
        event: super::DriverEvent,
        events: &tokio::sync::mpsc::UnboundedSender<super::DriverEvent>,
    ) -> bool {
        let _gate = self.close_result.lock().expect("session lifecycle");
        if !self.accepts(handle) {
            return false;
        }
        let mut approvals = self.pending_approvals.lock().expect("pending approvals");
        approvals.insert(id.clone(), pending);
        if events.send(event).is_err() {
            approvals.remove(&id);
            return false;
        }
        true
    }

    pub fn new(
        handle: SessionHandle,
        workspace: String,
        native_catalog: serde_json::Value,
        native: Arc<super::lifecycle::NativeProcess>,
    ) -> Arc<Self> {
        use std::sync::atomic::AtomicBool;
        Arc::new(Self {
            scope: Mutex::new(handle),
            workspace,
            admission: tokio::sync::Mutex::new(()),
            retired: tokio::sync::watch::channel(false).0,
            native_catalog,
            turn_active: AtomicBool::new(false),
            interrupt_requested: AtomicBool::new(false),
            cancel_tx: Mutex::new(None),
            interaction_tx: Mutex::new(None),
            pending_approvals: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            close_result: Mutex::new(None),
            turn_done: Mutex::new(None),
            native: Some(native),
        })
    }

    // Caller holds the session lifecycle lock.
    pub fn accepts(&self, handle: &SessionHandle) -> bool {
        use std::sync::atomic::Ordering;
        self.validate(handle).is_ok()
            && self.turn_active.load(Ordering::Acquire)
            && !self.closing.load(Ordering::Acquire)
            && !self.interrupt_requested.load(Ordering::Acquire)
    }
    pub fn handle(&self) -> SessionHandle {
        self.scope.lock().expect("session scope").clone()
    }
    pub fn validate(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        if self.handle().same_scope(handle) {
            Ok(())
        } else {
            Err(stale())
        }
    }
    pub async fn confirmed(&self) {
        if let Some(native) = &self.native {
            tokio::select! {
                ()=self.logical_confirmed()=>{},
                ()=native.confirmed()=>{},
            }
        } else {
            self.logical_confirmed().await;
        }
    }
    pub async fn logical_confirmed(&self) {
        let mut retired = self.retired.subscribe();
        loop {
            if *retired.borrow_and_update() {
                return;
            }
            if retired.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}
impl ZcodeDriver {
    pub(super) fn attempt(&self, state: &Arc<SessionState>, handle: &SessionHandle) -> Attempt {
        Attempt {
            connection: self.connection.clone(),
            sessions: self.sessions.clone(),
            state: state.clone(),
            handle: handle.clone(),
            retired: self.retired.clone(),
            committed: false,
        }
    }

    pub(super) fn restore_gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.restores.lock().expect("restore gates");
        gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = gates.get(id).and_then(std::sync::Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        gates.insert(id.into(), Arc::downgrade(&gate));
        gate
    }
}

/// Before a native identity arrives, an abandoned startup fences the original
/// private worker; no logical session ID can safely identify its partial work.
pub(super) struct NativeAttempt {
    pub native: Arc<super::lifecycle::NativeProcess>,
    pub committed: bool,
}
impl Drop for NativeAttempt {
    fn drop(&mut self) {
        if !self.committed {
            self.native.request_close();
        }
    }
}

/// Once identity is owned, abandon only that logical session on its original worker.
pub(super) struct Attempt {
    pub connection: Arc<tokio::sync::Mutex<Option<Connection>>>,
    pub sessions: Arc<std::sync::Mutex<HashMap<String, Arc<SessionState>>>>,
    pub state: Arc<SessionState>,
    pub handle: SessionHandle,
    pub retired: Retired,
    pub committed: bool,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.committed {
            let _ = super::lifecycle::close_session(
                self.connection.clone(),
                self.sessions.clone(),
                self.handle.clone(),
                self.state.clone(),
                self.retired.clone(),
            );
        }
    }
}
