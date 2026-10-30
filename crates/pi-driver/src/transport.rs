//! Pi RPC: correlate control replies by ID while continuously draining events.
use crate::events::Turn;
use agent_runtime::OwnedChild;
use driver_api::{DriverError, DriverEvent, SessionHandle};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::ChildStdin,
    sync::{mpsc, oneshot, watch},
};

const FRAME_LIMIT: u64 = 32 * 1024 * 1024;
type Reply = oneshot::Sender<Result<Value, DriverError>>;
#[derive(Default)]
pub(crate) struct State {
    pub scope: Option<SessionHandle>,
    pub latest_cache_hit: Option<f64>,
    pub request_usage: driver_api::usage::RequestUsageReport,
    pub pending: HashMap<String, Reply>,
    pub turn: Option<Turn>,
    pub disconnected: bool,
    pub(super) cancelling: bool,
    cancel_settled: bool,
    cancel_outcome: Option<watch::Receiver<Option<Result<(), DriverError>>>>,
}
pub(crate) struct Connection {
    pub admission: tokio::sync::Mutex<()>,
    input: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    pub(super) closing: watch::Sender<bool>,
    pub(super) cleanup: watch::Receiver<Option<Result<(), DriverError>>>,
    pub state: Arc<Mutex<State>>,
    pub(super) settled: Arc<tokio::sync::Notify>,
}

// Dropping a future does not undo bytes or a native model/config mutation.
// The supervisor retains the original process after this synchronous fence.
struct RetireOnCancel<'a> {
    connection: &'a Connection,
    armed: bool,
}
impl Drop for RetireOnCancel<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.connection.request_close();
        }
    }
}

struct PendingSend<'a> {
    state: &'a Mutex<State>,
    id: &'a str,
    transferred: bool,
}
impl Drop for PendingSend<'_> {
    fn drop(&mut self) {
        if !self.transferred {
            self.state.lock().expect("Pi state").pending.remove(self.id);
        }
    }
}

pub(crate) fn disconnected() -> DriverError {
    DriverError::Disconnected("Pi RPC connection closed".into())
}
impl State {
    pub(super) fn fail(&mut self, error: &DriverError) {
        self.disconnected = true;
        for (_, reply) in self.pending.drain() {
            let _ = reply.send(Err(error.clone()));
        }
        if let Some(turn) = self.turn.take() {
            let _ = turn.sender.send(Err(error.clone()));
        }
    }
}
impl Connection {
    pub fn spawn(
        executable: &Path,
        cwd: &str,
        session: Option<&str>,
        ephemeral: bool,
    ) -> Result<Self, DriverError> {
        let mut command = tokio::process::Command::new(executable);
        command
            .args(["--mode", "rpc"])
            .current_dir(cwd)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // Use Pi's own auth store. Only explicitly named Pi/provider configuration
        // enters the subprocess; never forward the host's complete environment.
        for key in [
            "PATH",
            "HOME",
            "LANG",
            "TZ",
            "TMPDIR",
            "TERM",
            "PI_CODING_AGENT_DIR",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "DEEPSEEK_API_KEY",
            "OPENROUTER_API_KEY",
            "XAI_API_KEY",
            "GROQ_API_KEY",
        ] {
            if let Some(value) = agent_runtime::inherited_variable(key) {
                command.env(key, value);
            }
        }
        agent_runtime::apply_proxy_environment(&mut command);
        if let Some(path) = agent_runtime::installed_node_path(executable) {
            command.env("PATH", path);
        }
        if ephemeral {
            command.arg("--no-session");
        }
        if let Some(path) = session {
            command.args(["--session", path]);
        }
        let mut child = OwnedChild::spawn(command)
            .map_err(|_| DriverError::NotConfigured("Pi executable unavailable".into()))?;
        let input = child.stdin.take().ok_or_else(disconnected)?;
        let stdout = child.stdout.take().ok_or_else(disconnected)?;
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let settled = Arc::new(tokio::sync::Notify::new());
        let signal = settled.clone();
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut generation_start: Option<std::time::Instant> = None;
            let error = loop {
                let mut bytes = Vec::new();
                match (&mut reader)
                    .take(FRAME_LIMIT + 1)
                    .read_until(b'\n', &mut bytes)
                    .await
                {
                    Ok(0) | Err(_) => break disconnected(),
                    Ok(_) if bytes.len() as u64 > FRAME_LIMIT => {
                        break DriverError::Protocol("Pi RPC frame exceeds limit".into());
                    }
                    Ok(_) => {}
                }
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    break DriverError::Protocol("Invalid Pi RPC JSON record".into());
                };
                let mut state = shared.lock().expect("Pi state");
                if value["type"] == "message_start" && value["message"]["role"] == "assistant" {
                    generation_start = None;
                }
                let update = &value["assistantMessageEvent"];
                if value["type"] == "message_update"
                    && matches!(
                        update["type"].as_str(),
                        Some("text_delta" | "thinking_delta" | "toolcall_delta")
                    )
                    && update["delta"].as_str().is_some_and(|s| !s.is_empty())
                {
                    generation_start.get_or_insert_with(std::time::Instant::now);
                }
                if value["type"] == "message_end" && value["message"]["role"] == "assistant" {
                    if let Some(mut record) = crate::request_usage::parse(&value["message"]) {
                        record.generation_ms = generation_start
                            .take()
                            .and_then(|s| u64::try_from(s.elapsed().as_millis()).ok())
                            .filter(|ms| *ms > 0);
                        if let Some(turn) = &state.turn {
                            let _ = turn.sender.send(Ok(driver_api::DriverEvent::UsageRecorded {
                                request: record.clone(),
                            }));
                        }
                        state.request_usage.observe(record);
                    }
                    generation_start = None;
                    let usage = &value["message"]["usage"];
                    let input = usage["input"]
                        .as_u64()
                        .zip(usage["cacheRead"].as_u64())
                        .zip(usage["cacheWrite"].as_u64())
                        .and_then(|((i, r), w)| i.checked_add(r)?.checked_add(w));
                    state.latest_cache_hit =
                        driver_api::usage::ratio(usage["cacheRead"].as_u64(), input);
                }
                if value["type"] == "response" {
                    if let Some(id) = value["id"].as_str()
                        && let Some(reply) = state.pending.remove(id)
                    {
                        let result = if value["success"] == true {
                            Ok(value["data"].clone())
                        } else {
                            Err(DriverError::Protocol(
                                "Pi rejected the RPC command; check native configuration".into(),
                            ))
                        };
                        let _ = reply.send(result);
                    }
                } else if value["type"] == "agent_settled" && state.cancelling {
                    state.cancel_settled = true;
                    signal.notify_one();
                } else if let Some(turn) = state.turn.as_mut()
                    && turn.observe(&value)
                {
                    state.turn = None;
                }
            };
            shared.lock().expect("Pi state").fail(&error);
            signal.notify_one();
        });
        let input = Arc::new(tokio::sync::Mutex::new(Some(input)));
        let (closing, close_rx) = watch::channel(false);
        let (completed, cleanup) = watch::channel(None);
        tokio::spawn(supervise(
            child,
            reader,
            input.clone(),
            state.clone(),
            settled.clone(),
            close_rx,
            completed,
        ));
        Ok(Self {
            admission: tokio::sync::Mutex::new(()),
            input,
            closing,
            cleanup,
            state,
            settled,
        })
    }
    pub async fn write(&self, value: &Value) -> Result<(), DriverError> {
        if self.state.lock().expect("Pi state").disconnected {
            return Err(disconnected());
        }
        let mut bytes = serde_json::to_vec(value)
            .map_err(|_| DriverError::Protocol("Invalid Pi command".into()))?;
        bytes.push(b'\n');
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut slot = self.input.lock().await;
            if self.state.lock().expect("Pi state").disconnected {
                return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
            }
            let input = slot
                .as_mut()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            let mut frame = RetireOnCancel {
                connection: self,
                armed: true,
            };
            input.write_all(&bytes).await?;
            input.flush().await?;
            frame.armed = false;
            Ok::<_, std::io::Error>(())
        })
        .await;
        if matches!(result, Ok(Ok(()))) {
            Ok(())
        } else {
            let _ = self.close().await;
            Err(disconnected())
        }
    }
    pub async fn call(&self, mut value: Value) -> Result<Value, DriverError> {
        #[cfg(test)]
        eprintln!(
            "Pi control start {}",
            value["type"].as_str().unwrap_or("unknown")
        );
        let receive = self.request(&mut value).await?;
        let mut pending = RetireOnCancel {
            connection: self,
            armed: true,
        };
        if let Ok(Ok(result)) = tokio::time::timeout(Duration::from_secs(30), receive).await {
            pending.armed = false;
            #[cfg(test)]
            eprintln!(
                "Pi control completed {}",
                value["type"].as_str().unwrap_or("unknown")
            );
            result
        } else {
            // A timed-out mutation may still execute. Invalidate the whole
            // process rather than reuse an unconfirmed model/session state.
            self.request_close();
            pending.armed = false;
            let _ = self.close().await;
            Err(DriverError::Disconnected("Pi RPC command timed out".into()))
        }
    }
    /// A cancelled statistics observer removes its own reply slot, not the
    /// running model turn. Partial native writes still fence the transport.
    pub async fn read_stats(&self) -> Result<Value, DriverError> {
        let mut value = json!({"type":"get_session_stats"});
        let received = self.request(&mut value).await?;
        let _pending = PendingSend {
            state: &self.state,
            id: value["id"].as_str().expect("Pi request id"),
            transferred: false,
        };
        tokio::time::timeout(Duration::from_secs(30), received)
            .await
            .map_err(|_| DriverError::Disconnected("Pi statistics query timed out".into()))?
            .map_err(|_| disconnected())?
    }

    pub async fn request(
        &self,
        value: &mut Value,
    ) -> Result<oneshot::Receiver<Result<Value, DriverError>>, DriverError> {
        let id = uuid::Uuid::new_v4().to_string();
        value["id"] = json!(id);
        let (reply, receive) = oneshot::channel();
        {
            let mut state = self.state.lock().expect("Pi state");
            if state.disconnected {
                return Err(disconnected());
            }
            state.pending.insert(id.clone(), reply);
        }
        let mut pending = PendingSend {
            state: &self.state,
            id: &id,
            transferred: false,
        };
        self.write(value).await?;
        pending.transferred = true;
        Ok(receive)
    }
    pub fn begin_turn(
        &self,
        handle: &SessionHandle,
    ) -> Result<
        (
            uuid::Uuid,
            mpsc::UnboundedReceiver<Result<DriverEvent, DriverError>>,
        ),
        DriverError,
    > {
        let mut state = self.state.lock().expect("Pi state");
        if !state
            .scope
            .as_ref()
            .is_some_and(|current| current.same_scope(handle))
        {
            return Err(super::sessions::stale_scope());
        }
        if state.disconnected {
            return Err(disconnected());
        }
        if state.turn.is_some() || state.cancelling {
            return Err(DriverError::Protocol("Pi turn already active".into()));
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        let turn = Turn::new(sender);
        let id = turn.id;
        state.turn = Some(turn);
        Ok((id, receiver))
    }
    /// Admit another turn only after acknowledgement, native settlement and an
    /// idle state read agree. A result acknowledgement alone does not stop Pi.
    pub async fn abort_turn(
        self: &Arc<Self>,
        handle: &SessionHandle,
        timeout: Duration,
    ) -> Result<(), DriverError> {
        let (mut outcome, start) = {
            let mut state = self.state.lock().expect("Pi state");
            if !state
                .scope
                .as_ref()
                .is_some_and(|current| current.same_scope(handle))
            {
                return Err(super::sessions::stale_scope());
            }
            if state.disconnected {
                return Err(disconnected());
            }
            if state.cancelling {
                (
                    state
                        .cancel_outcome
                        .as_ref()
                        .expect("cancellation receipt")
                        .clone(),
                    None,
                )
            } else {
                if state.turn.is_none() {
                    return Ok(());
                }
                state.cancelling = true;
                state.cancel_settled = false;
                let turn = state.turn.as_mut().expect("active turn");
                turn.interrupted = true;
                let ids = turn.dialogs.keys().cloned().collect::<Vec<_>>();
                let (completed, outcome) = watch::channel(None);
                state.cancel_outcome = Some(outcome.clone());
                (outcome, Some((ids, completed)))
            }
        };
        if let Some((ids, completed)) = start {
            let connection = self.clone();
            tokio::spawn(async move {
                let result = connection.abort_owned(timeout, ids).await;
                if result.is_err() {
                    connection.request_close();
                }
                completed.send_replace(Some(result));
            });
        }
        loop {
            if let Some(result) = outcome.borrow_and_update().clone() {
                return result;
            }
            if outcome.changed().await.is_err() {
                self.request_close();
                return Err(disconnected());
            }
        }
    }
    async fn abort_owned(&self, timeout: Duration, ids: Vec<String>) -> Result<(), DriverError> {
        tokio::time::timeout(timeout, async {
            for id in ids {
                self.write(&json!({"type":"extension_ui_response","id":id,"cancelled":true}))
                    .await?;
            }
            self.request(&mut json!({"type":"abort"}))
                .await?
                .await
                .map_err(|_| disconnected())??;
            loop {
                {
                    let state = self.state.lock().expect("Pi state");
                    if state.disconnected {
                        return Err(disconnected());
                    }
                    if state.cancel_settled {
                        break;
                    }
                }
                self.settled.notified().await;
            }
            let native = self
                .request(&mut json!({"type":"get_state"}))
                .await?
                .await
                .map_err(|_| disconnected())??;
            if native["isStreaming"] != false {
                return Err(DriverError::Protocol(
                    "Pi did not confirm cancellation settlement".into(),
                ));
            }
            let mut state = self.state.lock().expect("Pi state");
            if state.disconnected {
                return Err(disconnected());
            }
            if let Some(mut turn) = state.turn.take() {
                turn.observe(&json!({"type":"agent_settled"}));
            }
            state.cancelling = false;
            state.cancel_settled = false;
            Ok(())
        })
        .await
        .map_err(|_| DriverError::Disconnected("Pi cancellation did not settle in time".into()))?
    }

    pub fn request_close(&self) {
        self.state.lock().expect("Pi state").fail(&disconnected());
        self.closing.send_replace(true);
        self.settled.notify_one();
    }
    pub async fn close(&self) -> Result<(), DriverError> {
        self.request_close();
        let mut cleanup = self.cleanup.clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(result) = cleanup.borrow_and_update().clone() {
                    return result;
                }
                cleanup.changed().await.map_err(|_| {
                    DriverError::Disconnected("Pi cleanup ended without exit confirmation".into())
                })?;
            }
        })
        .await
        .map_err(|_| {
            DriverError::Disconnected("Pi process shutdown timed out; cleanup continues".into())
        })?
    }
    #[cfg(all(test, unix))]
    pub fn input_busy(&self) -> bool {
        self.input.try_lock().is_err()
    }
    #[cfg(all(test, unix))]
    pub fn cleanup(&self) -> watch::Receiver<Option<Result<(), DriverError>>> {
        self.cleanup.clone()
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.request_close();
    }
}

// This actor owns the OS process even if all callers cancel their wait. It
// never holds the input lock while requesting forceful native termination.
async fn supervise(
    mut child: OwnedChild,
    mut reader: tokio::task::JoinHandle<()>,
    input: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    state: Arc<Mutex<State>>,
    settled: Arc<tokio::sync::Notify>,
    mut closing: watch::Receiver<bool>,
    completed: watch::Sender<Option<Result<(), DriverError>>>,
) {
    tokio::select! {
        ()=async{while !reader.is_finished(){tokio::time::sleep(Duration::from_millis(10)).await;}}=>{},
        ()=async{while !*closing.borrow_and_update(){if closing.changed().await.is_err(){break;}}}=>{},
        _=child.wait()=>{},
    }
    state.lock().expect("Pi state").fail(&disconnected());
    settled.notify_one();
    if let Ok(mut input) = input.try_lock() {
        input.take();
    }
    // Pi flushes its native config/auth store on stdin EOF. Preserve the grace
    // period, but never interpret stdout EOF or a kill request as exit proof.
    if !matches!(
        tokio::time::timeout(Duration::from_secs(5), child.wait()).await,
        Ok(Ok(_))
    ) {
        let mut delay = 1;
        while child.terminate().await.is_err() {
            completed.send_replace(Some(Err(DriverError::Disconnected(
                "Pi process shutdown remains unconfirmed".into(),
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
    completed.send_replace(Some(Ok(())));
}
