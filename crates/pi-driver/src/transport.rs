//! Pi RPC: correlate control replies by ID while continuously draining events.
use crate::events::Turn;
use driver_api::{DriverError, DriverEvent};
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
    process::{Child, ChildStdin},
    sync::{mpsc, oneshot},
};

const FRAME_LIMIT: u64 = 32 * 1024 * 1024;
type Reply = oneshot::Sender<Result<Value, DriverError>>;
#[derive(Default)]
pub(crate) struct State {
    pub latest_cache_hit: Option<f64>,
    pub request_usage: driver_api::usage::RequestUsageReport,
    pub pending: HashMap<String, Reply>,
    pub turn: Option<Turn>,
    pub disconnected: bool,
    cancelling: bool,
    cancel_settled: bool,
}
pub(crate) struct Connection {
    input: tokio::sync::Mutex<Option<ChildStdin>>,
    child: tokio::sync::Mutex<Child>,
    reader: tokio::task::JoinHandle<()>,
    pub state: Arc<Mutex<State>>,
    settled: Arc<tokio::sync::Notify>,
}
pub(crate) fn disconnected() -> DriverError {
    DriverError::Disconnected("Pi RPC connection closed".into())
}
impl State {
    fn fail(&mut self, error: &DriverError) {
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
            if let Some(value) = std::env::var_os(key) {
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
        let mut child = command
            .spawn()
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
        Ok(Self {
            input: tokio::sync::Mutex::new(Some(input)),
            child: tokio::sync::Mutex::new(child),
            reader,
            state,
            settled,
        })
    }
    pub async fn write(&self, value: &Value) -> Result<(), DriverError> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|_| DriverError::Protocol("Invalid Pi command".into()))?;
        bytes.push(b'\n');
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut slot = self.input.lock().await;
            let input = slot
                .as_mut()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            input.write_all(&bytes).await?;
            input.flush().await
        })
        .await;
        if matches!(result, Ok(Ok(()))) {
            Ok(())
        } else {
            self.close().await;
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
        if let Ok(Ok(result)) = tokio::time::timeout(Duration::from_secs(30), receive).await {
            #[cfg(test)]
            eprintln!(
                "Pi control completed {}",
                value["type"].as_str().unwrap_or("unknown")
            );
            result
        } else {
            // A timed-out mutation may still execute. Invalidate the whole
            // process rather than reuse an unconfirmed model/session state.
            self.close().await;
            Err(DriverError::Disconnected("Pi RPC command timed out".into()))
        }
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
        self.write(value).await?;
        Ok(receive)
    }
    pub fn begin_turn(
        &self,
    ) -> Result<mpsc::UnboundedReceiver<Result<DriverEvent, DriverError>>, DriverError> {
        let mut state = self.state.lock().expect("Pi state");
        if state.disconnected {
            return Err(disconnected());
        }
        if state.turn.is_some() || state.cancelling {
            return Err(DriverError::Protocol("Pi turn already active".into()));
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        state.turn = Some(Turn::new(sender));
        Ok(receiver)
    }
    /// Admit another turn only after acknowledgement, native settlement and an
    /// idle state read agree. A result acknowledgement alone does not stop Pi.
    pub async fn abort_turn(&self, timeout: Duration) -> Result<(), DriverError> {
        let ids = {
            let mut state = self.state.lock().expect("Pi state");
            if state.disconnected {
                return Err(disconnected());
            }
            if state.turn.is_none() {
                return Ok(());
            }
            if state.cancelling {
                return Err(DriverError::Protocol(
                    "Pi cancellation already pending".into(),
                ));
            }
            state.cancelling = true;
            state.cancel_settled = false;
            let turn = state.turn.as_mut().expect("active turn");
            turn.interrupted = true;
            turn.dialogs.keys().cloned().collect::<Vec<_>>()
        };
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

    pub async fn close(&self) {
        self.state.lock().expect("Pi state").fail(&disconnected());
        // Pi disposes its runtime on stdin EOF. Killing a healthy metadata
        // process can leave native config/auth locks for the next startup.
        // ChildStdin::shutdown alone does not necessarily close the OS handle.
        // Taking/dropping it is what delivers EOF to the native RPC reader.
        drop(self.input.lock().await.take());
        let mut child = self.child.lock().await;
        if tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.reader.abort();
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
