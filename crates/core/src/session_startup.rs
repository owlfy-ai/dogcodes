//! One owned session acquisition. Losing its waiter does not drop native setup.
use crate::{
    Core, CoreError, history_bridge::HistoryLease, run_cleanup::Cleanup, run_control::Control,
};
use driver_api::{AgentDriver, SessionHandle};
use futures::FutureExt;
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};
use tokio::sync::{oneshot, watch};

#[derive(Clone)]
pub(super) enum Outcome {
    Pending,
    Ready(SessionHandle),
    Rejected,
    Closing(Arc<Cleanup>),
    Unknown,
}
pub(super) struct Startup {
    pub driver: Arc<dyn AgentDriver>,
    outcome: watch::Sender<Outcome>,
    history: Mutex<Option<Arc<HistoryLease>>>,
}
pub(super) struct Waiter<T> {
    startup: Arc<Startup>,
    control: Arc<Control>,
    response: oneshot::Receiver<Result<(SessionHandle, T), CoreError>>,
    consumed: bool,
}
impl<T> Waiter<T> {
    pub async fn result(mut self) -> Result<(SessionHandle, T), CoreError> {
        let result = (&mut self.response).await.unwrap_or_else(|_| {
            Err(CoreError::Internal(
                "native session startup observer ended without a result".into(),
            ))
        });
        self.consumed = true;
        if result.is_ok() {
            self.startup.release_history();
        }
        result
    }
}
impl<T> Drop for Waiter<T> {
    fn drop(&mut self) {
        if !self.consumed {
            self.control.request_stop();
        }
    }
}
impl Startup {
    pub fn start<T: Send + 'static>(
        core: Arc<Core>,
        control: Arc<Control>,
        driver: Arc<dyn AgentDriver>,
        history: Option<Arc<HistoryLease>>,
        operation: impl Future<Output = Result<(SessionHandle, T), CoreError>> + Send + 'static,
    ) -> Option<Waiter<T>> {
        let startup = Arc::new(Self {
            driver: driver.clone(),
            outcome: watch::channel(Outcome::Pending).0,
            history: Mutex::new(history),
        });
        if !control.register_startup(startup.clone()) {
            return None;
        }
        let (sender, response) = oneshot::channel();
        let owned = startup.clone();
        let original = control.clone();
        tokio::spawn(async move {
            let result = match AssertUnwindSafe(operation).catch_unwind().await {
                Ok(Ok((handle, value))) => {
                    match core.bind_run_session(&original.id, &driver, handle.clone()) {
                        Ok(()) => {
                            owned.outcome.send_replace(Outcome::Ready(handle.clone()));
                            Ok((handle, value))
                        }
                        Err(error) => {
                            let cleanup = Cleanup::start(
                                Some(driver),
                                Some(handle),
                                true,
                                std::time::Duration::from_secs(10),
                            );
                            owned
                                .outcome
                                .send_replace(Outcome::Closing(cleanup.clone()));
                            let retained = owned.clone();
                            let original = original.clone();
                            let core = core.clone();
                            tokio::spawn(async move {
                                cleanup.wait_confirmed().await;
                                retained.release_history();
                                // A lost owner cannot be allowed to release its
                                // workspace while this orphan is still alive.
                                if !core
                                    .owned_runs
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .contains_key(&original.id)
                                {
                                    original.finish();
                                }
                            });
                            Err(error)
                        }
                    }
                }
                Ok(Err(error)) => {
                    // Returned setup failures retain the adapter's cleanup
                    // contract. A lost/panicking future is deliberately different.
                    owned.release_history();
                    original.native_resolved();
                    owned.outcome.send_replace(Outcome::Rejected);
                    Err(error)
                }
                Err(_) => {
                    owned.outcome.send_replace(Outcome::Unknown);
                    Err(CoreError::Internal(
                        "native session startup worker ended unexpectedly".into(),
                    ))
                }
            };
            let _ = sender.send(result);
        });
        Some(Waiter {
            startup,
            control,
            response,
            consumed: false,
        })
    }
    pub async fn resolution(&self) -> Outcome {
        let mut outcome = self.outcome.subscribe();
        loop {
            let current = outcome.borrow_and_update().clone();
            if !matches!(current, Outcome::Pending) {
                return current;
            }
            if outcome.changed().await.is_err() {
                return Outcome::Unknown;
            }
        }
    }
    pub fn release_history(&self) {
        self.history.lock().expect("startup history lease").take();
    }
}

impl Core {
    pub(super) async fn mark_startup_source_retired(
        &self,
        task: &agent_domain::TaskId,
        run: &agent_domain::RunId,
        driver: &dyn AgentDriver,
        mapping: &agent_protocol::AgentSessionMapSummary,
    ) -> Result<(), CoreError> {
        let (kind, status, at) =
            if driver.resume_capability() == driver_api::SessionResumeCapability::Supported {
                ("session.resumable", "resumable", "resumableAt")
            } else {
                ("session.detached", "detached", "detachedAt")
            };
        self.append_session_identity(task,run,driver.executor(),Some(mapping),kind,serde_json::json!({
            "sessionMapId":mapping.session_map_id,"taskId":task,"nativeSessionId":mapping.native_session_id,
            "status":status,(at):chrono::Utc::now(),
        })).await
    }
}

#[cfg(test)]
mod tests;
