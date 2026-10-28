use super::{Connection, DriverError, PiDriver, Session, SessionHandle};
use std::sync::Arc;
use tokio::sync::watch;

pub(super) fn stale_scope() -> DriverError {
    DriverError::SessionIncompatible("Pi session handle is no longer current".into())
}

impl Connection {
    pub fn handle(&self) -> Option<SessionHandle> {
        self.state.lock().expect("Pi state").scope.clone()
    }

    pub fn validate_scope(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        if !self
            .handle()
            .is_some_and(|current| current.same_scope(handle))
        {
            return Err(stale_scope());
        }
        Ok(())
    }

    /// Caller holds admission across configuration and the subsequent handoff.
    pub fn handoff(&self, id: &str) -> Result<SessionHandle, DriverError> {
        let mut state = self.state.lock().expect("Pi state");
        if state.disconnected {
            return Err(super::transport::disconnected());
        }
        if state.turn.is_some() || state.cancelling {
            return Err(DriverError::Protocol("Pi session is busy".into()));
        }
        if state
            .scope
            .as_ref()
            .is_some_and(|handle| handle.native_session_id != id)
        {
            return Err(stale_scope());
        }
        let handle = SessionHandle::scoped(id.into());
        state.scope = Some(handle.clone());
        Ok(handle)
    }

    pub async fn confirmed(&self) {
        confirmed(self.cleanup.clone()).await;
    }

    pub fn observe_owner(&self, mut owner: watch::Receiver<bool>) {
        // Do not keep Connection alive: dropping an uninstalled candidate must
        // still request retirement even while its Driver continues to exist.
        let state = self.state.clone();
        let closing = self.closing.clone();
        let settled = self.settled.clone();
        let cleanup = self.cleanup.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = async {
                    while !*owner.borrow_and_update() {
                        if owner.changed().await.is_err() { break; }
                    }
                } => {
                    state.lock().expect("Pi state").fail(&super::transport::disconnected());
                    closing.send_replace(true);
                    settled.notify_one();
                },
                () = confirmed(cleanup) => {},
            }
        });
    }
}

async fn confirmed(mut cleanup: watch::Receiver<Option<Result<(), DriverError>>>) {
    loop {
        if matches!(*cleanup.borrow_and_update(), Some(Ok(()))) {
            return;
        }
        if cleanup.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

impl PiDriver {
    pub(super) async fn session_for(&self, handle: &SessionHandle) -> Result<Session, DriverError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(&handle.native_session_id)
            .ok_or_else(|| DriverError::SessionMissing("Pi session is not resident".into()))?;
        session.connection.validate_scope(handle)?;
        Ok(session.clone())
    }

    pub(super) async fn retire(
        &self,
        handle: &SessionHandle,
        connection: &Arc<Connection>,
    ) -> Result<(), DriverError> {
        connection.close().await?;
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(&handle.native_session_id)
            .is_some_and(|current| Arc::ptr_eq(&current.connection, connection))
        {
            sessions.remove(&handle.native_session_id);
            self.retired_handles
                .lock()
                .expect("Pi retired scopes")
                .insert(handle.native_session_id.clone(), handle.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod proof_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_cleanup_error_is_not_a_positive_exit_proof() {
        let (completed, cleanup) =
            watch::channel(Some(Err(super::super::transport::disconnected())));
        let mut proof = std::pin::pin!(confirmed(cleanup));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut proof)
                .await
                .is_err()
        );
        completed.send_replace(Some(Ok(())));
        tokio::time::timeout(Duration::from_secs(1), proof)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn supervisor_loss_does_not_prove_native_exit() {
        let (completed, cleanup) = watch::channel(None);
        drop(completed);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), confirmed(cleanup))
                .await
                .is_err()
        );
    }
}
