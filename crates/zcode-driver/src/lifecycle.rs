//! Ownership of the private CLI worker, separate from the sessions it serves.
use super::{Connection, DriverError, Incoming, SessionState, read_frames};
use agent_runtime::OwnedChild;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::ChildStdin,
    sync::{mpsc, watch},
};
type Outcome = Option<Result<(), DriverError>>;
pub(super) struct NativeProcess {
    #[cfg(all(test, unix))]
    pid: Option<u32>,
    input: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    stopping: Arc<AtomicBool>,
    close: watch::Sender<bool>,
    exit: watch::Receiver<Outcome>,
}

// A cancelled write_all may already have sent part of a JSON frame. Retire the
// private worker before releasing the input lock so nobody appends to that frame.
struct InputFrame<'a> {
    native: &'a NativeProcess,
    complete: bool,
}
impl Drop for InputFrame<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.native.request_close();
        }
    }
}

impl NativeProcess {
    pub fn start(
        mut child: OwnedChild,
        frames: mpsc::UnboundedSender<Incoming>,
    ) -> Result<Arc<Self>, DriverError> {
        let input = Arc::new(tokio::sync::Mutex::new(child.stdin.take()));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DriverError::Disconnected("ZCode stdout unavailable".into()))?;
        let reader = tokio::spawn(read_frames(stdout, frames));
        let stderr = child.stderr.take().map(|mut stderr| {
            tokio::spawn(async move {
                let mut bytes = [0; 4096];
                while let Ok(n) = stderr.read(&mut bytes).await {
                    if n == 0 {
                        break;
                    }
                }
            })
        });
        let stopping = Arc::new(AtomicBool::new(false));
        let (close, closing) = watch::channel(false);
        let (completed, exit) = watch::channel(None);
        let this = Arc::new(Self {
            #[cfg(all(test, unix))]
            pid: child.id(),
            input: input.clone(),
            stopping: stopping.clone(),
            close,
            exit,
        });
        // A weak reference keeps passive-exit publication from retaining the
        // close sender when every external owner has already disappeared.
        let original = Arc::downgrade(&this);
        tokio::spawn(async move {
            let mut closing = closing;
            let mut reader = reader;
            tokio::select! {
                ()=async{while !reader.is_finished(){tokio::time::sleep(Duration::from_millis(10)).await;}}=>{},
                ()=async{while !*closing.borrow_and_update(){if closing.changed().await.is_err(){break;}}}=>{},
                _=child.wait()=>{},
            }
            stopping.store(true, Ordering::Release);
            if let Some(original) = original.upgrade() {
                // Credential helpers observe this same fence for requested
                // shutdown, actual EOF and passive original-leader exit.
                original.close.send_replace(true);
            }
            if let Ok(mut input) = input.try_lock() {
                input.take();
            }
            if !matches!(
                tokio::time::timeout(Duration::from_secs(2), child.wait()).await,
                Ok(Ok(_))
            ) {
                let mut delay = 1;
                while child.terminate().await.is_err() {
                    completed.send_replace(Some(Err(DriverError::Disconnected(
                        "ZCode worker exit remains unconfirmed".into(),
                    ))));
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    delay = (delay * 2).min(30);
                }
            }
            input.lock().await.take();
            if tokio::time::timeout(Duration::from_secs(1), &mut reader)
                .await
                .is_err()
            {
                reader.abort();
                let _ = reader.await;
            }
            if let Some(stderr) = stderr {
                stderr.abort();
                let _ = stderr.await;
            }
            completed.send_replace(Some(Ok(())));
        });
        Ok(this)
    }
    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }
    pub fn closing(&self) -> watch::Receiver<bool> {
        self.close.subscribe()
    }
    pub fn request_close(&self) {
        self.stopping.store(true, Ordering::Release);
        self.close.send_replace(true);
    }
    pub async fn closed(&self) -> Result<(), DriverError> {
        wait(self.exit.clone(), Duration::from_secs(8)).await
    }
    pub async fn confirmed(&self) {
        let mut exit = self.exit.clone();
        loop {
            if matches!(*exit.borrow_and_update(), Some(Ok(()))) {
                return;
            }
            if exit.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
    pub async fn close(&self) -> Result<(), DriverError> {
        self.request_close();
        self.closed().await
    }
    pub async fn send(&self, value: &Value) -> Result<(), DriverError> {
        if self.stopping() {
            return Err(DriverError::Disconnected("ZCode worker is closing".into()));
        }
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut input = self.input.lock().await;
            if self.stopping() {
                return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
            }
            let input = input
                .as_mut()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            let mut frame = InputFrame {
                native: self,
                complete: false,
            };
            input.write_all(value.to_string().as_bytes()).await?;
            input.write_all(b"\n").await?;
            input.flush().await?;
            frame.complete = true;
            Ok::<_, std::io::Error>(())
        })
        .await;
        if matches!(result, Ok(Ok(()))) {
            Ok(())
        } else {
            self.request_close();
            Err(DriverError::Disconnected(
                "ZCode worker input failed; cleanup continues".into(),
            ))
        }
    }
    pub fn observe_owner(self: &Arc<Self>, mut owner: watch::Receiver<bool>) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut exit = this.exit.clone();
            loop {
                if *owner.borrow() {
                    this.request_close();
                    break;
                }
                if matches!(*exit.borrow(), Some(Ok(()))) {
                    break;
                }
                tokio::select! {
                    changed=owner.changed()=>if changed.is_err(){this.request_close();break;},
                    changed=exit.changed()=>if changed.is_err(){break;},
                }
            }
        });
    }
    #[cfg(all(test, unix))]
    pub fn input_busy(&self) -> bool {
        self.input.try_lock().is_err()
    }
    #[cfg(all(test, unix))]
    pub fn id(&self) -> Option<u32> {
        self.pid
    }
}
pub(super) async fn wait(
    mut outcome: watch::Receiver<Outcome>,
    timeout: Duration,
) -> Result<(), DriverError> {
    tokio::time::timeout(timeout, async {
        loop {
            if let Some(result) = outcome.borrow_and_update().clone() {
                return result;
            }
            outcome.changed().await.map_err(|_| {
                DriverError::Disconnected("ZCode operation ended without confirmation".into())
            })?;
        }
    })
    .await
    .map_err(|_| {
        DriverError::Disconnected("ZCode operation is still pending confirmation".into())
    })?
}

pub(super) type Receipt = watch::Receiver<Outcome>;
type Sessions = Arc<std::sync::Mutex<HashMap<String, Arc<SessionState>>>>;
pub(super) fn close_session(
    connection: Arc<tokio::sync::Mutex<Option<Connection>>>,
    sessions: Sessions,
    handle: driver_api::SessionHandle,
    state: Arc<SessionState>,
    retired: super::scopes::Retired,
) -> Result<Receipt, DriverError> {
    let id = handle.native_session_id.clone();
    let (outcome, completed) = {
        let mut close = state.close_result.lock().expect("close receipt");
        state.validate(&handle)?;
        if let Some(outcome) = close.as_ref() {
            return Ok(outcome.clone());
        }
        state.closing.store(true, Ordering::Release);
        state.interrupt_requested.store(true, Ordering::Release);
        if let Some(cancel) = state.cancel_tx.lock().expect("cancel sender").as_ref() {
            let _ = cancel.send(());
        }
        let (completed, outcome) = watch::channel(None);
        *close = Some(outcome.clone());
        (outcome, completed)
    };
    tokio::spawn(async move {
        let mut sent = false;
        let mut delay = 1;
        loop {
            let result = async {
                if let Some(native) = &state.native
                    && native.stopping()
                {
                    return native.closed().await;
                }
                let mut guard = connection.lock().await;
                let current = guard.as_mut().ok_or_else(|| {
                    DriverError::Disconnected("ZCode original connection is unavailable".into())
                })?;
                if let Some(native) = &state.native
                    && !Arc::ptr_eq(native, &current.native)
                {
                    return native.closed().await;
                }
                if !sent {
                    sent = true;
                    match current
                        .request("session/close", json!({"sessionId":id}), None)
                        .await
                    {
                        Ok(value) if value["closed"] == true => return Ok(()),
                        Ok(value) if value["closed"] == false => {
                            // The vendor explicitly performed no close. Allow a
                            // fresh user attempt; unknown outcomes never resend.
                            {
                                let mut close = state.close_result.lock().expect("close receipt");
                                close.take();
                            }
                            return Err(DriverError::CapabilityUnavailable(
                                "ZCode refused to close this session".into(),
                            ));
                        }
                        Err(DriverError::SessionMissing(_)) => return Ok(()),
                        _ => {}
                    }
                }
                // session/list includes persisted history and can omit deferred
                // live sessions. Only this runtime's session/read unavailable
                // response is evidence that the original native session is gone.
                match current
                    .request(
                        "session/read",
                        json!({"sessionId":id,"contentProfile":"index","messageLimit":1}),
                        None,
                    )
                    .await
                {
                    Err(DriverError::SessionMissing(_)) => Ok(()),
                    _ => Err(DriverError::Disconnected(
                        "ZCode session close remains unconfirmed".into(),
                    )),
                }
            }
            .await;
            match result {
                Ok(()) => {
                    let mut sessions = sessions.lock().expect("sessions");
                    if sessions
                        .get(&id)
                        .is_some_and(|current| Arc::ptr_eq(current, &state))
                    {
                        sessions.remove(&id);
                        retired
                            .lock()
                            .expect("retired scopes")
                            .insert(id.clone(), handle.clone());
                    }
                    state.retired.send_replace(true);
                    completed.send_replace(Some(Ok(())));
                    break;
                }
                Err(error) => {
                    let rejected = matches!(error, DriverError::CapabilityUnavailable(_));
                    completed.send_replace(Some(Err(error)));
                    if rejected {
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        }
    });
    Ok(outcome)
}
