//! Business-session ownership on a shared native ACP connection. Advertised
//! session/close retires native resources without disturbing peer owners. Agents
//! without that capability retain local retirement and last-owner scope cleanup.
use super::{Command, ConnectionHandle, DriverError, SessionHandle, Shared, lifecycle, turn};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::{OwnedMutexGuard, oneshot, watch};

mod native;
use native::retire_native;

pub(super) struct Resident {
    pub handle: SessionHandle,
    pub epoch: Arc<AtomicBool>,
    pub auxiliary: AtomicBool,
    pub native_closed: AtomicBool,
    native_close: bool,
    closing_active: AtomicBool,
    native: Option<Arc<lifecycle::Control>>,
    pub turn: Mutex<Option<Arc<turn::Turn>>>,
    closing: Mutex<Option<turn::Receipt>>,
}
impl Resident {
    pub fn new(id: String, connection: &ConnectionHandle, native_close: bool) -> Arc<Self> {
        Arc::new(Self {
            handle: SessionHandle::scoped(id),
            epoch: connection.finished.clone(),
            auxiliary: AtomicBool::new(false),
            native_closed: AtomicBool::new(false),
            native_close,
            closing_active: AtomicBool::new(false),
            native: connection.native.clone(),
            turn: Mutex::new(None),
            closing: Mutex::new(None),
        })
    }
    pub fn is_closing(&self) -> bool {
        self.closing.lock().expect("ACP close").is_some()
    }
    pub fn shutdown_confirmed(&self) -> bool {
        self.epoch.load(Ordering::Acquire)
            || self
                .closing
                .lock()
                .expect("ACP close")
                .as_ref()
                .is_some_and(|r| matches!(*r.borrow(), Some(Ok(()))))
    }
    pub fn uncertain_close(&self) -> bool {
        self.is_closing()
            && !self.epoch.load(Ordering::Acquire)
            && (self.closing_active.load(Ordering::Acquire)
                || self
                    .turn
                    .lock()
                    .expect("ACP session turn")
                    .as_ref()
                    .is_some_and(|turn| !matches!(*turn.done.borrow(), Some(Ok(())))))
    }
}

impl Shared {
    /// Caller holds lifecycle; a native ID alone is not execution authority.
    pub fn validate_handle(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        if !self.reconnect_required.load(Ordering::Acquire)
            && self
                .sessions
                .lock()
                .expect("ACP sessions")
                .get(&handle.native_session_id)
                .is_some_and(|r| r.handle.same_scope(handle) && !r.epoch.load(Ordering::Acquire))
        {
            Ok(())
        } else {
            Err(DriverError::SessionIncompatible(
                "ACP session ownership changed; restore the session before operating".into(),
            ))
        }
    }

    /// A completed original close is retryable, but cannot close a replacement.
    pub fn validate_close_handle(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        if let Some(r) = self
            .sessions
            .lock()
            .expect("ACP sessions")
            .get(&handle.native_session_id)
        {
            return if r.handle.same_scope(handle) {
                Ok(())
            } else {
                Err(DriverError::SessionIncompatible(
                    "ACP close targets a replaced session".into(),
                ))
            };
        }
        if self
            .retired_handles
            .lock()
            .expect("ACP retired handles")
            .get(&handle.native_session_id)
            .is_some_and(|h| h.same_scope(handle))
        {
            Ok(())
        } else {
            Err(DriverError::SessionMissing(
                "ACP close has no owned session".into(),
            ))
        }
    }
    // lifecycle orders this decision with new setup reservations and retirement.
    pub fn may_stop_connection(&self, session: &str, epoch: &Arc<AtomicBool>) -> bool {
        !self.setup_in_progress.load(Ordering::Acquire)
            && !self
                .sessions
                .lock()
                .expect("ACP sessions")
                .iter()
                .any(|(id, resident)| {
                    id != session && Arc::ptr_eq(&resident.epoch, epoch) && !resident.is_closing()
                })
    }
    pub fn setup_available(&self) -> Result<(), DriverError> {
        self.prompt_available()?;
        if self
            .sessions
            .lock()
            .expect("ACP sessions")
            .values()
            .any(|resident| resident.is_closing() && !resident.epoch.load(Ordering::Acquire))
        {
            return Err(DriverError::CapabilityUnavailable(
                "ACP session retirement is still pending".into(),
            ));
        }
        Ok(())
    }
    pub fn prompt_available(&self) -> Result<(), DriverError> {
        if self.turn_active.load(Ordering::Acquire)
            || self
                .sessions
                .lock()
                .expect("ACP sessions")
                .values()
                .any(|resident| resident.uncertain_close())
            || self
                .latest_turn
                .lock()
                .expect("ACP turn")
                .as_ref()
                .is_some_and(|turn| turn.failed_stop() && !turn.epoch.load(Ordering::Acquire))
        {
            return Err(DriverError::CapabilityUnavailable(
                "ACP session work is still pending confirmation".into(),
            ));
        }
        Ok(())
    }
}

/// Completion remains owned even if the caller disappears after a native
/// new/load/config request has been sent. The setup gate is never released in
/// the middle of that transaction; an abandoned result retires its own session.
pub(super) async fn finish_setup(
    shared: Arc<Shared>,
    connection: ConnectionHandle,
    response: oneshot::Receiver<Result<super::SessionId, DriverError>>,
    permit: OwnedMutexGuard<()>,
    target: Option<String>,
) -> Result<SessionHandle, DriverError> {
    let (reply, result) = oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let native = response
            .await
            .unwrap_or_else(|_| Err(super::driver_error_from_channel()));
        let result = {
            let _gate = shared.lifecycle.lock().expect("ACP lifecycle");
            let result = native.and_then(|session| {
                if connection.unavailable() {
                    return Err(super::driver_error_from_channel());
                }
                let id = session.to_string();
                if id.is_empty() {
                    return Err(DriverError::Protocol("ACP session id missing".into()));
                }
                if target.is_none()
                    && shared
                        .sessions
                        .lock()
                        .expect("ACP sessions")
                        .get(&id)
                        .is_some_and(|r| Arc::ptr_eq(&r.epoch, &connection.finished))
                {
                    // A failed new must never replace a peer's ownership or
                    // close that peer by the colliding native ID. Its native
                    // configuration is now uncertain; explicit restore must
                    // establish a fresh physical scope before more work.
                    shared.reconnect_required.store(true, Ordering::Release);
                    return Err(DriverError::Protocol(
                        "ACP session/new reused an already owned session ID".into(),
                    ));
                }
                let resident = Resident::new(
                    id.clone(),
                    &connection,
                    shared.native_close.load(Ordering::Acquire),
                );
                let handle = resident.handle.clone();
                shared
                    .sessions
                    .lock()
                    .expect("ACP sessions")
                    .insert(id.clone(), resident);
                shared
                    .closed_sessions
                    .lock()
                    .expect("closed sessions")
                    .remove(&id);
                let mut latest = shared.latest_turn.lock().expect("ACP turn");
                if latest
                    .as_ref()
                    .is_some_and(|turn| !Arc::ptr_eq(&turn.epoch, &connection.finished))
                {
                    latest.take();
                }
                shared
                    .retired_handles
                    .lock()
                    .expect("ACP retired handles")
                    .remove(&id);
                Ok(handle)
            });
            if result.is_err()
                && let Some(target) = &target
            {
                shared
                    .closed_sessions
                    .lock()
                    .expect("closed sessions")
                    .insert(target.clone());
            }
            shared.setup_in_progress.store(false, Ordering::Release);
            result
        };
        if result.is_err()
            && let Some(target) = &target
        {
            // Native load/config may have partly succeeded before an error.
            // The old handle must stop routing before returning that error.
            if shared
                .sessions
                .lock()
                .expect("ACP sessions")
                .contains_key(target)
            {
                let _ = begin_close(shared.clone(), target, Duration::from_secs(10)).await;
            }
        }
        if let Err(Ok(abandoned)) = reply.send(result) {
            // Still own setup admission, so no last-owner decision or restore
            // can race registration and retirement of this abandoned result.
            let _ = begin_close(
                shared,
                &abandoned.native_session_id,
                Duration::from_secs(10),
            )
            .await;
        }
    });
    result
        .await
        .unwrap_or_else(|_| Err(super::driver_error_from_channel()))
}

/// The caller owns setup admission. Only admission holds the physical-connection
/// mutex; waiting for stop/cache/exit proofs happens in an independent task.
pub(super) async fn begin_close(
    shared: Arc<Shared>,
    id: &str,
    timeout: Duration,
) -> Result<turn::Receipt, DriverError> {
    let mut connection = shared.connection.lock().await;
    let (resident, terminal, finished, stop, close_physical, original) = {
        let _gate = shared.lifecycle.lock().expect("ACP lifecycle");
        let resident = shared
            .sessions
            .lock()
            .expect("ACP sessions")
            .get(id)
            .cloned();
        let Some(resident) = resident else {
            if shared
                .closed_sessions
                .lock()
                .expect("closed sessions")
                .contains(id)
            {
                return Ok(watch::channel(Some(Ok(()))).1);
            }
            return Err(DriverError::SessionMissing(
                "ACP session is not owned by this driver".into(),
            ));
        };
        let mut current = resident.closing.lock().expect("ACP close");
        if let Some(receipt) = current.as_ref() {
            return Ok(receipt.clone());
        }
        let (terminal, receipt) = watch::channel(None);
        *current = Some(receipt.clone());
        drop(current);
        shared
            .closed_sessions
            .lock()
            .expect("closed sessions")
            .insert(id.into());
        let active = shared
            .active_session
            .lock()
            .expect("active session")
            .as_deref()
            == Some(id);
        if resident.native_close && active {
            resident.closing_active.store(true, Ordering::Release);
            if let Some(turn) = resident.turn.lock().expect("ACP session turn").as_ref() {
                turn.retiring.store(true, Ordering::Release);
            }
        }
        let original = connection
            .as_ref()
            .filter(|current| {
                resident.native_close && Arc::ptr_eq(&current.finished, &resident.epoch)
            })
            .cloned();
        let stop = if active {
            shared.interrupt_requested.store(true, Ordering::Release);
            shared.cancel_interactions(id);
            resident
                .turn
                .lock()
                .expect("ACP session turn")
                .as_ref()
                .map(|turn| turn.stop(timeout))
        } else {
            None
        };
        let close_physical = shared.may_stop_connection(id, &resident.epoch);
        if close_physical {
            if !resident.native_close
                && let Some(native) = &resident.native
            {
                native.request_close();
            }
            if let Some(original) = connection.as_ref()
                && Arc::ptr_eq(&original.finished, &resident.epoch)
            {
                *connection = Some(original.cleanup_marker());
            }
        }
        (resident, terminal, receipt, stop, close_physical, original)
    };
    drop(connection);
    let id = id.to_owned();
    tokio::spawn(async move {
        // Keep the stop request alive but do not equate its timeout/error with
        // retirement. Another owner is allowed to close while this waits; its
        // last-owner exit proof also settles this original retirement.
        let _stop = stop;
        if resident.native_close {
            retire_native(
                shared,
                id,
                resident,
                terminal,
                original,
                close_physical,
                timeout,
            )
            .await;
            return;
        }
        loop {
            if resident.epoch.load(Ordering::Acquire) {
                break;
            }
            let quiescent = resident
                .turn
                .lock()
                .expect("ACP session turn")
                .as_ref()
                .is_none_or(|turn| matches!(*turn.done.borrow(), Some(Ok(()))));
            if !close_physical && quiescent {
                let response = {
                    let current = shared.connection.lock().await;
                    current
                        .as_ref()
                        .filter(|current| {
                            Arc::ptr_eq(&current.finished, &resident.epoch)
                                && !current.unavailable()
                        })
                        .and_then(|current| {
                            let (reply, response) = oneshot::channel();
                            current
                                .commands
                                .send(Command::ForgetSession {
                                    session_id: id.clone(),
                                    reply,
                                })
                                .ok()
                                .map(|()| response)
                        })
                };
                if let Some(mut response) = response {
                    loop {
                        tokio::select! {
                            result = &mut response => { if result.is_ok() { finish(&shared, &id, &resident, &terminal).await; return; } break; },
                            () = tokio::time::sleep(Duration::from_millis(20)) => { if resident.epoch.load(Ordering::Acquire) { break; } }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        finish(&shared, &id, &resident, &terminal).await;
    });
    Ok(finished)
}

async fn finish(
    shared: &Shared,
    id: &str,
    resident: &Arc<Resident>,
    completed: &watch::Sender<turn::Outcome>,
) {
    let mut connection = shared.connection.lock().await;
    let _gate = shared.lifecycle.lock().expect("ACP lifecycle");
    if resident.epoch.load(Ordering::Acquire)
        && connection
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(&current.finished, &resident.epoch))
    {
        connection.take();
    }
    let mut sessions = shared.sessions.lock().expect("ACP sessions");
    if sessions
        .get(id)
        .is_some_and(|current| Arc::ptr_eq(current, resident))
    {
        sessions.remove(id);
        shared
            .retired_handles
            .lock()
            .expect("ACP retired handles")
            .insert(id.to_owned(), resident.handle.clone());
    }
    if resident.native_close {
        let original = resident.turn.lock().expect("ACP session turn");
        let mut latest = shared.latest_turn.lock().expect("ACP turn");
        if original
            .as_ref()
            .zip(latest.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
        {
            latest.take();
        }
    }
    completed.send_replace(Some(Ok(())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcpDriver, AcpLaunch, SessionResumeCapability};
    use driver_api::AgentDriver;

    #[tokio::test]
    async fn shutdown_waits_for_setup_before_marking_a_session_closed() {
        let driver = AcpDriver::new(
            "cursor",
            &agent_domain::InstallationId::new().to_string(),
            AcpLaunch {
                executable: "unused-fixture".into(),
                args: vec![],
            },
            None,
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let session = SessionHandle::scoped("pending-restore".into());
        let permit = driver.shared.setup.lock().await;
        let closing = driver.shutdown(session.clone());
        tokio::pin!(closing);
        // No process is needed: even handle validation must wait for setup.
        assert!(futures::poll!(&mut closing).is_pending());
        assert!(driver.shared.closed_sessions.lock().unwrap().is_empty());
        drop(permit);
        let result = tokio::time::timeout(Duration::from_secs(1), closing)
            .await
            .expect("shutdown must resume after setup admission is released");
        assert!(matches!(result, Err(DriverError::SessionMissing(_))));
    }
}
