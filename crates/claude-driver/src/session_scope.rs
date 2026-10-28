use super::{ClaudeDriver, DriverError, Ordering, SessionHandle, SessionProcess, lifecycle};
use std::sync::Arc;

pub(super) fn stale_scope() -> DriverError {
    DriverError::SessionIncompatible("Claude session handle is no longer current".into())
}

/// Once native I/O may have started, abandoning a caller must fence the
/// original process rather than admit another operation into unknown state.
pub(super) struct NativeAttempt<'a> {
    process: &'a SessionProcess,
    confirmed: bool,
}

impl<'a> NativeAttempt<'a> {
    pub fn new(process: &'a SessionProcess) -> Self {
        Self {
            process,
            confirmed: false,
        }
    }
    pub fn confirm(&mut self) {
        self.confirmed = true;
    }
}

impl Drop for NativeAttempt<'_> {
    fn drop(&mut self) {
        if !self.confirmed {
            self.process.request_close();
        }
    }
}

impl SessionProcess {
    pub fn handle(&self) -> SessionHandle {
        self.scope.lock().expect("Claude session scope").clone()
    }

    pub fn validate_scope(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        if !self.handle().same_scope(handle) {
            return Err(stale_scope());
        }
        Ok(())
    }

    /// Retain admission across async writes/configuration so a hot reuse cannot
    /// move authority between validation and the actual native effect.
    pub async fn admit(
        &self,
        handle: &SessionHandle,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, DriverError> {
        let admission = self.admission.lock().await;
        self.validate_scope(handle)?;
        if self.closing.load(Ordering::Acquire) || self.native.stopping() {
            return Err(DriverError::Disconnected(
                "Claude session is closing".into(),
            ));
        }
        Ok(admission)
    }

    pub fn request_close(&self) {
        if !self.closing.swap(true, Ordering::AcqRel) {
            let _ = self.commands.send(lifecycle::Command::Close);
        }
        self.native.request_close();
    }

    pub async fn retired(&self) {
        self.native.confirmed().await;
        while !self.cleanup_done.load(Ordering::Acquire) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

impl Drop for SessionProcess {
    fn drop(&mut self) {
        self.native.request_close();
    }
}

impl Drop for ClaudeDriver {
    fn drop(&mut self) {
        self.stopping.send_replace(true);
    }
}

impl ClaudeDriver {
    pub(super) async fn process_for(
        &self,
        handle: &SessionHandle,
    ) -> Result<Arc<SessionProcess>, DriverError> {
        let processes = self.processes.lock().await;
        let process = processes
            .get(&handle.native_session_id)
            .ok_or_else(|| DriverError::SessionMissing("Claude session is not active".into()))?;
        process.validate_scope(handle)?;
        Ok(process.clone())
    }

    pub(super) async fn remove_retired(
        &self,
        handle: &SessionHandle,
        process: &Arc<SessionProcess>,
    ) {
        let mut processes = self.processes.lock().await;
        if processes
            .get(&handle.native_session_id)
            .is_some_and(|current| Arc::ptr_eq(current, process))
        {
            processes.remove(&handle.native_session_id);
            self.retired_handles
                .lock()
                .expect("Claude retired scopes")
                .insert(handle.native_session_id.clone(), handle.clone());
        }
    }
}
