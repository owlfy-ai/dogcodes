//! `ZCode` app-server product driver.
//!
//! Drives the `ZCode` desktop app's bundled CLI (`zcode.cjs`) over its
//! "`ZCode` Protocol" stdio surface. The protocol is a `JSON-RPC` variant: `NDJSON`
//! frames with `id`/`method`/`params` requests and `result`/`error` responses,
//! but **without** the `jsonrpc` version field (sending one is a validation
//! `error` — verified against 0.16.5). The server also initiates requests
//! toward the client (`session/requestRuntimePreferences` during session
//! creation, `interaction/requestPermission` for approvals), which this driver
//! answers inline.
//!
//! Launch is dependency-free: the desktop app's background Electron helper runs the
//! Node script via `ELECTRON_RUN_AS_NODE=1`, so no system Node.js is needed.
//!
//! Approval requests preserve the vendor option ids and stay parked until the
//! Hub answers through `respond_to_interaction`; session resume is capability
//! gated. Model configuration comes from the native Desktop provider registry.
//! Native credentials remain local; vendor configuration files are never modified.
//! Native account grants are projected before catalog/session creation. Runtime
//! credentials are scoped to the same account and supplied over local pipes only.

mod account;
#[cfg(all(test, unix))]
mod cleanup_tests;
mod lifecycle;
mod metadata;
mod models;
mod open;
mod resume;
#[cfg(test)]
mod resume_tests;
#[cfg(all(test, unix))]
mod scope_tests;
mod scopes;
mod session_usage;
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_domain::{ExecutorRef, InstallationId, ProviderId};
use async_trait::async_trait;
use driver_api::{
    AgentDriver, ApprovalOption, ApprovalSemanticRole, ContextDeliveryKind, ContextUsageSnapshot,
    DriverError, DriverEvent, DriverEventStream, PromptRequest as HubPromptRequest, QuotaProvider,
    QuotaSnapshot, SessionHandle, SessionRequest, SessionResumeCapability,
};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{ChildStdout, Command},
    sync::mpsc,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Versions verified against the installed CLI using an isolated native provider and history.
#[must_use]
pub fn verified_resume_capability(version: Option<&str>) -> SessionResumeCapability {
    if version == Some("0.16.9") {
        SessionResumeCapability::Supported
    } else {
        SessionResumeCapability::Unknown
    }
}

const FRAME_LIMIT: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const CONTEXT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Environment variables handed to the child; everything else is stripped so
/// credentials never leak into the agent process. `ELECTRON_RUN_AS_NODE` makes
/// the desktop app's binary act as the Node runtime for the bundled CLI.
const ENV_ALLOWLIST: &[&str] = &["PATH", "HOME", "LANG", "TZ", "TMPDIR", "TERM"];

/// How the app bundle is addressed: the Electron binary plus the CLI script.
#[derive(Clone, Debug)]
pub struct ZcodeLaunch {
    /// The desktop app's main binary (e.g. `ZCode.app/Contents/MacOS/ZCode`).
    pub app_binary: PathBuf,
    /// The bundled Node CLI script (`.../Resources/glm/zcode.cjs`).
    pub cli_script: PathBuf,
}

impl ZcodeLaunch {
    fn bundled_provider_config(&self) -> Option<PathBuf> {
        // 0.16.9's CLI searches glm/provider and a source-tree path, but the
        // desktop ships this file in Resources/config/provider. Supply only
        // the bundled seed: the CLI still owns user config and runtime refresh.
        let resources = self.cli_script.parent()?.parent()?;
        let config = resources.join("config/provider/zcode-builtin.json");
        config.is_file().then_some(config)
    }
}
mod quota;
#[cfg(all(test, unix))]
mod readiness_tests;

#[derive(Debug)]
enum Incoming {
    /// Response to one of our requests.
    Response {
        id: Value,
        ok: Option<Value>,
        error: Option<Value>,
    },
    /// Server-initiated request that we answer inline.
    ServerRequest {
        id: Value,
        method: String,
        params: Value,
    },
    /// Notification (no `id`).
    Notification { method: String, params: Value },
}

fn parse_frame(line: &str) -> Option<Incoming> {
    let value: Value = serde_json::from_str(line).ok()?;
    let id = value.get("id").cloned();
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
    match (id, method) {
        (Some(id), Some(method)) => Some(Incoming::ServerRequest { id, method, params }),
        (Some(id), None) => Some(Incoming::Response {
            id,
            ok: value.get("result").cloned(),
            error: value.get("error").cloned(),
        }),
        (None, Some(method)) => Some(Incoming::Notification { method, params }),
        (None, None) => None,
    }
}

/// Maps vendor notifications onto driver events. Shapes verified against the
/// bundle's own notification registry; anything unmapped stays a `ProtocolWarning`
/// without payload content.
fn map_notification(method: &str, params: &Value) -> Option<DriverEvent> {
    let payload = params.get("payload").unwrap_or(params);
    match event_method(method, params) {
        "stream.delta" => {
            if payload
                .get("parentToolCallId")
                .and_then(Value::as_str)
                .is_some_and(|p| !p.is_empty())
            {
                return None;
            }
            if payload.get("kind").and_then(Value::as_str) == Some("text_delta") {
                let delta = payload
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !delta.is_empty() {
                    return Some(DriverEvent::TextDelta {
                        item_id: "message".into(),
                        text: delta.to_owned(),
                    });
                }
            }
            None
        }
        "turn.started" => Some(DriverEvent::TurnStarted {
            native_turn_id: payload
                .get("turnId")
                .or_else(|| params.get("turnId"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "tool.updated" => {
            let kind = payload.get("kind").and_then(Value::as_str)?;
            let name = payload
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let id = payload
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            match kind {
                "started" | "scheduled" => Some(DriverEvent::ToolStarted {
                    item_id: id.to_owned(),
                    tool_name: name.to_owned(),
                    summary: "tool step started".into(),
                }),
                "completed" | "result" => Some(DriverEvent::ToolCompleted {
                    item_id: id.to_owned(),
                    tool_name: name.to_owned(),
                    summary: "tool step completed".into(),
                    success: true,
                }),
                "failed" | "error" => Some(DriverEvent::ToolCompleted {
                    item_id: id.to_owned(),
                    tool_name: name.to_owned(),
                    summary: "tool step failed".into(),
                    success: false,
                }),
                _ => None,
            }
        }
        // Turn terminals are consumed by run_turn; this is only a fallback
        // for events arriving outside an active turn.
        "turn.completed"
        | "model.completed"
        | "turn.failed"
        | "state.updated"
        | "process/mcpTelemetry"
        | "process/resourceSample"
        | "v4/telemetry/event"
        | "computer-use/operation-event"
        | "session/event"
        | "session.created"
        | "session.resumed"
        | "session.titleUpdated"
        | "session.updated"
        | "session.closed"
        | "permission.requested"
        | "permission.resolved" => None,
        _ => Some(DriverEvent::ProtocolWarning {
            message: "ZCode emitted an unrecognized notification; payload omitted".into(),
        }),
    }
}

fn stable_turn_failure(payload: &Value) -> DriverEvent {
    let error = payload.get("error").unwrap_or(payload);
    let kind = error.get("code").or_else(|| error.get("type"));
    let kind = kind.and_then(|value| {
        value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_i64().map(|value| value.to_string()))
    });
    if kind.as_deref() == Some("1308") {
        return DriverEvent::TurnQuotaExhausted {
            resets_at_unix: error
                .get("next_flush_time")
                .or_else(|| error.get("nextFlushTime"))
                .and_then(Value::as_i64),
        };
    }
    DriverEvent::TurnFailed {
        reason: kind.map_or_else(
            || "ZCode turn ended with an error result".into(),
            |kind| format!("ZCode turn failed ({kind})"),
        ),
    }
}

fn event_method<'a>(method: &'a str, params: &'a Value) -> &'a str {
    if method != "session/event" {
        return method;
    }
    // 0.16.5 supplies an authoritative top-level event type. Keep the key
    // signature fallback for older/local variants that omit it.
    if let Some(event_type) = params.get("type").and_then(Value::as_str) {
        return match event_type {
            "model.streaming" => "stream.delta",
            other => other,
        };
    }
    let payload = params.get("payload").unwrap_or(params);
    if payload.get("resultType").is_some() {
        "turn.completed"
    } else if payload.get("stopReason").is_some() {
        "model.completed"
    } else if let Some(kind) = payload.get("kind").and_then(Value::as_str) {
        match kind {
            "started" | "completed" | "failed" | "scheduled" | "result" | "error" | "progress"
            | "batch" => "tool.updated",
            _ => "stream.delta",
        }
    } else if payload.get("turnNumber").is_some() && payload.get("input").is_some() {
        "turn.started"
    } else if payload.get("error").is_some() && payload.get("turnPhase").is_some() {
        "turn.failed"
    } else {
        method
    }
}

fn session_id_from_snapshot(value: &Value) -> Option<&str> {
    value
        .pointer("/session/sessionId")
        .or_else(|| value.get("sessionId"))
        .and_then(Value::as_str)
}

async fn read_native_catalog(
    connection: &mut Connection,
    workspace: &std::path::Path,
) -> Result<Value, DriverError> {
    let state = connection
        .request(
            "workspace/readState",
            json!({"workspace":{"workspacePath":workspace,"workspaceKey":workspace}}),
            None,
        )
        .await;
    let state = match state {
        Ok(state) => state,
        Err(DriverError::CapabilityUnavailable(_)) => {
            // 0.16.9 removed workspace/readState; session snapshots now
            // expose the registry-owned model catalog. No model turn is run.
            let state = connection
                .request(
                    "session/create",
                    json!({
                        "workspace":{"workspacePath":workspace,"workspaceKey":workspace},
                        "persistence":"deferred"
                    }),
                    None,
                )
                .await?;
            let id = session_id_from_snapshot(&state).ok_or_else(|| {
                DriverError::Protocol("zcode model probe is missing sessionId".into())
            })?;
            connection
                .request(
                    "session/close",
                    json!({"sessionId":id,"expectedPersistence":"deferred"}),
                    None,
                )
                .await?;
            state
        }
        Err(error) => return Err(error),
    };
    Ok(state)
}

fn context_from_snapshot(value: &Value, catalog: &Value) -> Option<ContextUsageSnapshot> {
    let usage = session_usage::read(value, catalog);
    Some(ContextUsageSnapshot {
        used_tokens: usage.context_used_tokens?,
        window_tokens: usage.context_window_tokens?,
        cache_hit_rate_percent: usage.cache_hit_rate_percent,
    })
}

struct TurnContent {
    text: String,
    attachments: Vec<Value>,
}

struct TurnResult {
    terminal: DriverEvent,
}

struct PendingApproval {
    server_id: Value,
    option_ids: Vec<String>,
    responses: HashMap<String, Value>,
}

struct InteractionDecision {
    server_id: Value,
    response: Value,
}

fn permission_request_event(params: &Value) -> Option<(String, DriverEvent, Vec<String>)> {
    let native_request_id = params
        .get("requestId")
        .or_else(|| params.get("toolCallId"))
        .and_then(Value::as_str)?
        .to_owned();
    let options = params.get("options")?.as_array()?;
    let mut option_ids = Vec::with_capacity(options.len());
    let approval_options = options
        .iter()
        .filter_map(|option| {
            let option_id = option.get("optionId")?.as_str()?.to_owned();
            option_ids.push(option_id.clone());
            Some(ApprovalOption {
                option_id,
                label: option
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("ZCode option")
                    .to_owned(),
                semantic_role: match option.get("kind").and_then(Value::as_str) {
                    Some("allow_once") => ApprovalSemanticRole::ApproveOnce,
                    Some("allow_always") => ApprovalSemanticRole::ApprovePersistent,
                    Some("deny") => ApprovalSemanticRole::Reject,
                    _ => ApprovalSemanticRole::Other,
                },
            })
        })
        .collect::<Vec<_>>();
    if approval_options.is_empty() {
        return None;
    }
    let event = DriverEvent::ApprovalRequested {
        native_request_id: native_request_id.clone(),
        tool_name: params
            .get("toolName")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_owned(),
        command_summary: params
            .get("input")
            .map(Value::to_string)
            .unwrap_or_default(),
        options: approval_options,
    };
    Some((native_request_id, event, option_ids))
}

/// The runtime preferences the driver reports when the server asks. Only
/// fields verified by the 0.16.5 schema are provided; everything else keeps
/// the vendor default.
fn runtime_preferences() -> Value {
    json!({ "nativeSearchEnhancementsEnabled": false })
}

struct Connection {
    native: Arc<lifecycle::NativeProcess>,
    owner: Option<tokio::sync::watch::Receiver<bool>>,
    frames: mpsc::UnboundedReceiver<Incoming>,
    next_request_id: u64,
    request_confirmed: bool,
    seen_turns: HashMap<String, std::collections::HashSet<String>>,
    #[cfg(test)]
    request_timeout: std::time::Duration,
    account: Option<account::Account>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.native.request_close();
    }
}

impl Connection {
    fn spawn(launch: &ZcodeLaunch) -> Result<Self, DriverError> {
        let mut command = Command::new(
            agent_runtime::electron_node_executable(&launch.app_binary).map_err(|_| {
                DriverError::NotConfigured("ZCode background runtime is unavailable".into())
            })?,
        );
        command
            .arg(&launch.cli_script)
            .arg("app-server")
            .env_clear()
            .env("ELECTRON_RUN_AS_NODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        agent_runtime::apply_platform_environment(&mut command);
        let account = account::Account::new(launch);
        if account.is_some() {
            command.args(["--stdio", "--surface", "desktop"]);
        }
        for variable in ENV_ALLOWLIST {
            if let Some(value) = agent_runtime::inherited_variable(variable) {
                command.env(variable, value);
            }
        }
        for variable in [
            "ZCODE_DATA_BASE_DIR",
            "ZCODE_DESKTOP_HOME_DIR",
            "USERPROFILE",
            "SYSTEMROOT",
            "WINDIR",
        ] {
            if let Some(value) = agent_runtime::inherited_variable(variable) {
                command.env(variable, value);
            }
        }
        if let Some(config) = launch.bundled_provider_config() {
            command.env("ZCODE_BUILTIN_PROVIDER_CONFIG_FILE", config);
            if let Some(home) = std::env::var_os("ZCODE_DATA_BASE_DIR")
                .or_else(|| std::env::var_os("HOME"))
                .or_else(|| std::env::var_os("USERPROFILE"))
            {
                command.env(
                    "ZCODE_PERSONAL_PROVIDER_CONFIG_FILE",
                    PathBuf::from(home).join(".zcode/v2/provider_config.json"),
                );
            }
        }
        agent_runtime::apply_proxy_environment(&mut command);
        let child = agent_runtime::OwnedChild::spawn(command).map_err(|error| {
            if matches!(error.kind(), std::io::ErrorKind::NotFound) {
                DriverError::NotConfigured("ZCode app binary is missing".into())
            } else {
                DriverError::Disconnected(format!("zcode app-server failed to start: {error}"))
            }
        })?;
        let (frames_tx, frames) = mpsc::unbounded_channel();
        let native = lifecycle::NativeProcess::start(child, frames_tx)?;
        Ok(Self {
            native,
            owner: None,
            frames,
            next_request_id: 1,
            request_confirmed: false,
            seen_turns: HashMap::new(),
            #[cfg(test)]
            request_timeout: REQUEST_TIMEOUT,
            account,
        })
    }
    fn observe_owner(&mut self, owner: tokio::sync::watch::Receiver<bool>) {
        self.native.observe_owner(owner.clone());
        self.owner = Some(owner);
    }
    async fn send(&mut self, value: &Value) -> Result<(), DriverError> {
        if self
            .owner
            .as_ref()
            .is_some_and(|owner| *owner.borrow() || owner.has_changed().is_err())
        {
            return Err(DriverError::Disconnected("ZCode driver is closing".into()));
        }
        self.native.send(value).await
    }
    async fn start_request(&mut self, method: &str, params: Value) -> Result<Value, DriverError> {
        let id = json!(self.next_request_id);
        self.next_request_id += 1;
        self.send(&json!({ "id": id, "method": method, "params": params }))
            .await?;
        Ok(id)
    }

    /// Sends a request and waits for its response, answering server-initiated
    /// requests that arrive while we wait (preferences, approvals).
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        events: Option<&mpsc::UnboundedSender<DriverEvent>>,
    ) -> Result<Value, DriverError> {
        self.request_confirmed = false;
        if matches!(
            method,
            "session/create" | "session/resume" | "workspace/readState"
        ) {
            self.refresh_account(false).await?;
        }
        self.request_inner(method, params, events).await
    }

    async fn request_inner(
        &mut self,
        method: &str,
        params: Value,
        events: Option<&mpsc::UnboundedSender<DriverEvent>>,
    ) -> Result<Value, DriverError> {
        self.request_confirmed = false;
        let id = self.start_request(method, params).await?;
        #[cfg(not(test))]
        let timeout = REQUEST_TIMEOUT;
        #[cfg(test)]
        let timeout = self.request_timeout;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let frame = tokio::time::timeout_at(deadline, self.frames.recv())
                .await
                .map_err(|_| DriverError::Protocol(format!("zcode {method} response timed out")))?
                .ok_or_else(|| DriverError::Disconnected("zcode app-server stream ended".into()))?;
            match frame {
                Incoming::Response {
                    id: response_id,
                    ok,
                    error,
                } if response_id == id => {
                    self.request_confirmed = true;
                    if let Some(error) = error {
                        return Err(map_request_error(method, &error));
                    }
                    return Ok(ok.unwrap_or_else(|| json!({})));
                }
                Incoming::ServerRequest {
                    id: server_id,
                    method: server_method,
                    params: server_params,
                } => {
                    let response = if server_method == "interaction/requestProviderRuntimeHeaders" {
                        self.provider_auth(&server_params).await
                    } else {
                        answer_server_request(&server_method, &server_params, events)
                    };
                    if let Some(event) = notification_event(&server_method)
                        && let Some(events) = events
                    {
                        let _ = events.send(event);
                    }
                    let _ = self
                        .send(&json!({ "id": server_id, "result": response }))
                        .await;
                }
                Incoming::Notification { method, params } => {
                    if let Some(event) = map_notification(event_method(&method, &params), &params)
                        && let Some(events) = events
                    {
                        let _ = events.send(event);
                    }
                }
                // Late responses to requests we no longer wait for.
                Incoming::Response { .. } => {}
            }
        }
    }

    /// Runs one accepted prompt until the protocol emits its real terminal
    /// notification. `session/send` itself only acknowledges queueing, so
    /// returning on that response would truncate streaming and fake success.
    async fn run_turn(
        &mut self,
        content: TurnContent,
        events: &mpsc::UnboundedSender<DriverEvent>,
        mut cancel: mpsc::UnboundedReceiver<()>,
        mut interactions: mpsc::UnboundedReceiver<InteractionDecision>,
        state: &SessionState,
        handle: &SessionHandle,
    ) -> Result<TurnResult, DriverError> {
        let session_id = handle.native_session_id.as_str();
        self.refresh_account(false).await?;
        let send_id = self
            .start_request(
                "session/send",
                json!({ "sessionId": session_id, "content": content.text, "attachments": content.attachments }),
            )
            .await?;
        #[cfg(not(test))]
        let control_timeout = REQUEST_TIMEOUT;
        #[cfg(test)]
        let control_timeout = self.request_timeout;
        let start_deadline = tokio::time::Instant::now() + control_timeout;
        let mut stop_deadline = None;
        let mut stop_id = None;
        let mut cancel_open = true;
        let mut current_turn_started = false;
        let mut native_turn: Option<String> = None;

        loop {
            tokio::select! {
                biased;
                cancel_message = cancel.recv(), if cancel_open && stop_id.is_none() => {
                    match cancel_message {
                        Some(()) => {
                            stop_id = Some(self.start_request(
                                "session/stop",
                                json!({ "sessionId": session_id }),
                            ).await?);
                            stop_deadline=Some(tokio::time::Instant::now()+control_timeout);
                        }
                        None => cancel_open = false,
                    }
                }
                decision = interactions.recv() => {
                    if let Some(decision) = decision {
                        let current={let _gate=state.close_result.lock().expect("session lifecycle");state.accepts(handle)};
                        if !current {continue;}
                        self.send(&json!({
                            "id": decision.server_id,
                            "result": decision.response
                        })).await?;
                    }
                }
                ()=async{if let Some(deadline)=stop_deadline{tokio::time::sleep_until(deadline).await;}else if !current_turn_started{tokio::time::sleep_until(start_deadline).await;}else{std::future::pending::<()>().await;}}=>{
                    return Err(DriverError::Protocol("ZCode start/stop was not confirmed in time".into()));
                }
                frame = self.frames.recv() => {
                    let frame = frame.ok_or_else(|| {
                        DriverError::Disconnected("zcode app-server stream ended".into())
                    })?;
                    match frame {
                        Incoming::Response { id, ok: _, error } if id == send_id => {
                            if let Some(error) = error {
                                return Err(map_request_error("session/send", &error));
                            }
                        }
                        Incoming::Response { id, error, .. }
                            if stop_id.as_ref().is_some_and(|stop| *stop == id) =>
                        {
                            if error.is_some() {
                                return Err(DriverError::Protocol(
                                    "zcode session/stop request failed".into(),
                                ));
                            }
                            // stop only signals AbortController; its RPC reply
                            // is not proof that the turn or tools have ended.
                        }
                        Incoming::ServerRequest {
                            id,
                            method,
                            params,
                        } => {
                            let foreign=label_differs(&params,"sessionId",Some(session_id))||label_differs(&params,"turnId",native_turn.as_deref());
                            let current={let _gate=state.close_result.lock().expect("session lifecycle");state.accepts(handle)};
                            if foreign||!current {
                                let response=match method.as_str(){
                                    "interaction/requestUserInput"=>json!({"action":"cancel"}),
                                    "interaction/requestProviderRuntimeHeaders"=>json!({"headersApplied":false,"errorMessage":"Original turn is no longer available"}),
                                    _=>json!({"decision":"deny"}),
                                };
                                self.send(&json!({"id":id,"result":response})).await?;
                                continue;
                            }
                            if method == "interaction/requestUserInput" {
                                match driver_api::questions::native_form("zcode",&params){
                                    Ok(form)=>{
                                        let request_id=format!("zcode-question-{}",InstallationId::new());
                                        let pending=PendingApproval {server_id:id.clone(),option_ids:vec![],responses:HashMap::from([("question".into(),params)])};
                                        if !state.publish(handle,request_id.clone(),pending,DriverEvent::QuestionRequested {native_request_id:request_id,form},events) {
                                            self.send(&json!({"id":id,"result":{"action":"cancel"}})).await?;
                                        }
                                    },
                                    Err(_)=>self.send(&json!({"id":id,"result":{"action":"cancel"}})).await?,
                                }
                                continue;
                            }
                            if method == "interaction/requestPermission" {
                                let Some((_, mut event, option_ids)) =
                                    permission_request_event(&params)
                                else {
                                    self.send(&json!({
                                        "id": id,
                                        "result": { "decision": "deny" }
                                    })).await?;
                                    continue;
                                };
                                let native_request_id=format!("zcode-approval-{}",InstallationId::new());
                                if let DriverEvent::ApprovalRequested {native_request_id:id,..}=&mut event {id.clone_from(&native_request_id);}
                                let pending=PendingApproval {
                                    server_id:id.clone(),
                                    responses:params["options"].as_array().into_iter().flatten().filter_map(|option|Some((option["optionId"].as_str()?.into(),option.get("response")?.clone()))).collect(),
                                    option_ids,
                                };
                                if !state.publish(handle,native_request_id,pending,event,events) {
                                    self.send(&json!({"id":id,"result":{"decision":"deny"}})).await?;
                                }
                                continue;
                            }
                            let response = if method == "interaction/requestProviderRuntimeHeaders" {
                                // Account verification can wait on a user challenge; cancellation
                                // must still stop the native task and drop its private helper.
                                tokio::select! {
                                    biased;
                                    message = cancel.recv(), if cancel_open && stop_id.is_none() => {
                                        if message.is_some() {
                                            stop_id = Some(self.start_request("session/stop", json!({"sessionId":session_id})).await?);
                                            stop_deadline=Some(tokio::time::Instant::now()+control_timeout);
                                        } else { cancel_open = false; }
                                        json!({"headersApplied":false,"errorMessage":"账号验证已取消。"})
                                    },
                                    value = self.provider_auth(&params) => value,
                                }
                            } else {
                                answer_server_request(&method, &params, Some(events))
                            };
                            if let Some(event) = notification_event(&method) {
                                let _ = events.send(event);
                            }
                            self.send(&json!({ "id": id, "result": response })).await?;
                        }
                        Incoming::Notification { method, params } => {
                            if label_differs(&params,"sessionId",Some(session_id)){continue;}
                            let event_method = event_method(&method, &params);
                            if event_method == "turn.started" {
                                if current_turn_started {continue;}
                                if let Some(id)=native_label(&params,"turnId") {
                                    if label_differs(&params,"turnId",Some(id)){continue;}
                                    if !self.seen_turns.entry(session_id.into()).or_default().insert(id.into()) {continue;}
                                    native_turn=Some(id.into());
                                }
                                current_turn_started = true;
                            } else if label_differs(&params,"turnId",native_turn.as_deref()) {continue;}
                            match event_method {
                                "turn.completed" => {
                                    if !current_turn_started {
                                        continue;
                                    }
                                    let payload = params.get("payload").unwrap_or(&params);
                                    let result_type = payload
                                        .get("resultType")
                                        .and_then(Value::as_str)
                                        .unwrap_or("success");
                                    let terminal = match result_type {
                                        "success" => DriverEvent::TurnCompleted {
                                            summary: "ZCode turn completed".into(),
                                        },
                                        "cancelled" => DriverEvent::TurnInterrupted {
                                            summary: "ZCode turn was interrupted".into(),
                                        },
                                        _ => DriverEvent::TurnFailed {
                                            reason: format!("ZCode turn ended with {result_type}"),
                                        },
                                    };
                                    return Ok(TurnResult {
                                        terminal,
                                    });
                                }
                                "turn.failed" => {
                                    if !current_turn_started {
                                        continue;
                                    }
                                    let payload = params.get("payload").unwrap_or(&params);
                                    return Ok(TurnResult {
                                        terminal: stable_turn_failure(payload),
                                        });
                                }
                                _ => {
                                    for event in driver_api::presentation::presentation_events("zcode", event_method, &params) { let _ = events.send(event); }
                                    if let Some(event) =
                                        map_notification(event_method, &params)
                                    {
                                        let _ = events.send(event);
                                    }
                                }
                            }
                        }
                        // Other late responses belong to completed control-plane calls.
                        Incoming::Response { .. } => {}
                    }
                }
            }
        }
    }
}

fn native_label<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str).or_else(|| {
        params
            .get("payload")
            .and_then(|p| p.get(key))
            .and_then(Value::as_str)
    })
}
fn label_differs(params: &Value, key: &str, expected: Option<&str>) -> bool {
    expected.is_some_and(|expected| {
        [
            params.get(key),
            params.get("payload").and_then(|p| p.get(key)),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|value| value != expected)
    })
}

/// Reads stdout lines into the shared frame channel until EOF.
async fn read_frames(stdout: ChildStdout, frames: mpsc::UnboundedSender<Incoming>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut bytes = Vec::new();
        let limit = u64::try_from(FRAME_LIMIT + 1).expect("frame limit");
        match (&mut reader)
            .take(limit)
            .read_until(b'\n', &mut bytes)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) if bytes.len() > FRAME_LIMIT => break,
            Ok(_) => {}
        }
        let Ok(line) = std::str::from_utf8(&bytes) else {
            break;
        };
        if let Some(frame) = parse_frame(line.trim_end())
            && frames.send(frame).is_err()
        {
            break;
        }
    }
}

/// Answers a server-initiated request that is not parked by an active turn.
/// Active-turn approvals are handled by `run_turn`; this fallback declines
/// stray approval requests using the verified vendor response shape.
fn answer_server_request(
    method: &str,
    _params: &Value,
    events: Option<&mpsc::UnboundedSender<DriverEvent>>,
) -> Value {
    match method {
        "session/requestRuntimePreferences" => runtime_preferences(),
        "interaction/requestPermission" => {
            // Fallback for permission requests outside an active turn. During
            // a turn, run_turn parks the server request for Hub interaction.
            if let Some(events) = events {
                let _ = events.send(DriverEvent::ProtocolWarning {
                    message: "a ZCode permission request was declined by the hub default".into(),
                });
            }
            json!({ "decision": "deny" })
        }
        "interaction/requestUserInput" => {
            if let Some(events) = events {
                let _ = events.send(DriverEvent::ProtocolWarning {
                    message: "a ZCode user-input request was declined by the hub default".into(),
                });
            }
            json!({ "action": "cancel" })
        }
        "interaction/requestProviderRuntimeHeaders" => json!({
            "headersApplied": false,
            "errorMessage": "请在 ZCode 中登录并重新检测 Agent。"
        }),
        // Unknown server requests get an empty ack; the schema treats missing
        // fields as defaults and the turn keeps running.
        _ => json!({}),
    }
}

fn notification_event(method: &str) -> Option<DriverEvent> {
    let _ = method;
    None
}

/// Maps a `JSON-RPC` `error` object to a stable `DriverError`. Vendor text is never
/// propagated; known `error` signatures become stable kinds.
fn map_request_error(method: &str, error: &Value) -> DriverError {
    if error.get("code").and_then(Value::as_i64) == Some(-32601) {
        return DriverError::CapabilityUnavailable(format!("zcode method unavailable: {method}"));
    }
    if error.pointer("/data/code").and_then(Value::as_str)
        == Some("ZCODE_RUNTIME_MODEL_UNAVAILABLE")
    {
        return DriverError::NotConfigured(
            "ZCode restored model runtime is unavailable; check its native configuration".into(),
        );
    }
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    if matches!(method, "session/read" | "session/close")
        && error["code"].as_i64() == Some(-32004)
        && message.starts_with("Session is not active:")
    {
        return DriverError::SessionMissing("ZCode session is not active in this worker".into());
    }

    let data_text = error
        .get("data")
        .map(std::string::ToString::to_string)
        .unwrap_or_default();
    if method == "session/resume"
        && (error.pointer("/data/code").and_then(Value::as_str) == Some("session_not_found")
            || message.starts_with("Session not found:")
            || message.starts_with("v4 command session not found:"))
    {
        return DriverError::SessionMissing("zcode native session does not exist".into());
    }
    // The verified "run zcode login" signature from 0.16.5.
    if message.contains("Model config is missing") || data_text.contains("Model config is missing")
    {
        return DriverError::NotConfigured(
            "zcode requires a one-time `zcode login` to create its model config".into(),
        );
    }
    if message.contains("auth") || message.contains("Auth") || data_text.contains("auth_failed") {
        return DriverError::NotConfigured("zcode authentication is missing or expired".into());
    }
    DriverError::Protocol(format!("zcode {method} request failed"))
}

pub struct ZcodeDriver {
    quota_cache: tokio::sync::Mutex<quota::Cache>,
    catalog_probe: Arc<tokio::sync::Mutex<()>>,
    executor: ExecutorRef,
    launch: ZcodeLaunch,
    native_version: Option<String>,
    resume_capability: SessionResumeCapability,
    connection: Arc<tokio::sync::Mutex<Option<Connection>>>,
    stopping: tokio::sync::watch::Sender<bool>,
    sessions: Arc<std::sync::Mutex<HashMap<String, Arc<SessionState>>>>,
    retired: scopes::Retired,
    restores: std::sync::Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    /// Timestamp of the last successful `usage/stats` pull; a short TTL
    /// keeps polling cheap (the call is a local agent-db query, zero model).
    usage_pulled_at_unix_ms: std::sync::Mutex<Option<i64>>,
    /// Latest context reading per native session. Context is task-scoped and
    /// must never bleed across conversations that share one installation.
    usage: Arc<driver_api::usage::UsageStore>,
    latest_context: Arc<std::sync::Mutex<HashMap<String, ContextUsageSnapshot>>>,
}

struct SessionState {
    scope: std::sync::Mutex<SessionHandle>,
    workspace: String,
    admission: tokio::sync::Mutex<()>,
    retired: tokio::sync::watch::Sender<bool>,
    native_catalog: Value,
    turn_active: AtomicBool,
    interrupt_requested: AtomicBool,
    cancel_tx: std::sync::Mutex<Option<mpsc::UnboundedSender<()>>>,
    interaction_tx: std::sync::Mutex<Option<mpsc::UnboundedSender<InteractionDecision>>>,
    pending_approvals: std::sync::Mutex<HashMap<String, PendingApproval>>,
    closing: AtomicBool,
    close_result: std::sync::Mutex<Option<lifecycle::Receipt>>,
    turn_done: std::sync::Mutex<Option<lifecycle::Receipt>>,
    native: Option<Arc<lifecycle::NativeProcess>>,
}

impl SessionState {
    fn finish_turn(&self, handle: &SessionHandle, failed: bool) {
        let _gate = self.close_result.lock().expect("session lifecycle");
        if self.validate(handle).is_err() {
            return;
        }
        if failed {
            self.closing.store(true, Ordering::Release);
        }
        self.interrupt_requested.store(false, Ordering::Release);
        self.cancel_tx.lock().expect("cancel sender").take();
        self.interaction_tx
            .lock()
            .expect("interaction sender")
            .take();
        self.pending_approvals
            .lock()
            .expect("pending approvals")
            .clear();
        self.turn_active.store(false, Ordering::Release);
    }
}
impl Drop for ZcodeDriver {
    fn drop(&mut self) {
        self.stopping.send_replace(true);
    }
}
impl ZcodeDriver {
    /// Creates a driver for one concrete installation. `resume_capability`
    /// must come from real runtime evidence, never from documentation.
    pub fn new(
        provider_id: &str,
        installation_id: &str,
        launch: ZcodeLaunch,
        native_version: Option<String>,
        resume_capability: SessionResumeCapability,
    ) -> Result<Self, agent_domain::DomainError> {
        Ok(Self {
            quota_cache: tokio::sync::Mutex::new(quota::Cache::default()),
            catalog_probe: Arc::default(),
            executor: ExecutorRef {
                provider_id: ProviderId::parse(provider_id)?,
                installation_id: Some(
                    InstallationId::parse(installation_id)
                        .map_err(|error| {
                            agent_domain::DomainError::InvalidProviderId(error.to_string())
                        })?
                        .to_string(),
                ),
            },
            launch,
            native_version,
            resume_capability,
            connection: Arc::new(tokio::sync::Mutex::new(None)),
            sessions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            retired: Arc::default(),
            restores: std::sync::Mutex::new(HashMap::new()),
            stopping: tokio::sync::watch::channel(false).0,
            usage_pulled_at_unix_ms: std::sync::Mutex::new(None),
            usage: Arc::default(),
            latest_context: Arc::new(std::sync::Mutex::new(HashMap::new())),
        })
    }

    /// Zero-model-call readiness probe: `session/list` proves the protocol
    /// surface; a `session/create` attempt in a temp workspace surfaces the
    /// "Model config is missing" signature as `NotConfigured` (run
    /// `zcode login`). The deferred session needs an explicit close ACK before
    /// returning; cancellation cannot save it as a persistent conversation.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::NotConfigured`] when the login/config step is
    /// missing and [`DriverError::Protocol`] for protocol-surface failures.
    pub async fn probe_ready(launch: &ZcodeLaunch) -> Result<(), DriverError> {
        let connection = Self::ready_connection(launch).await?;
        connection.native.close().await
    }

    /// Keeps the authenticated process after closing the temporary probe session.
    ///
    /// # Errors
    ///
    /// Returns an error when the installation identity, native protocol, or
    /// configured model fails readiness verification.
    pub async fn connect_ready(
        provider_id: &str,
        installation_id: &str,
        launch: ZcodeLaunch,
        native_version: Option<String>,
        resume_capability: SessionResumeCapability,
    ) -> Result<Self, DriverError> {
        let driver = Self::new(
            provider_id,
            installation_id,
            launch,
            native_version,
            resume_capability,
        )
        .map_err(|_| DriverError::Protocol("invalid ZCode installation identity".into()))?;
        let mut connection = Self::ready_connection(&driver.launch).await?;
        connection.observe_owner(driver.stopping.subscribe());
        *driver.connection.lock().await = Some(connection);
        Ok(driver)
    }

    async fn ready_connection(launch: &ZcodeLaunch) -> Result<Connection, DriverError> {
        let mut connection = Connection::spawn(launch)?;
        let result = async {
            let list = tokio::time::timeout(
                PROBE_TIMEOUT,
                connection.request("session/list", json!({}), None),
            )
            .await
            .map_err(|_| {
                DriverError::Protocol("zcode did not answer session/list in time".into())
            })??;
            if list.get("sessions").and_then(Value::as_array).is_none() {
                return Err(DriverError::Protocol(
                    "zcode session/list response is missing the sessions array".into(),
                ));
            }
            // Close the temp session if creation succeeded; a NotConfigured error
            // is the expected honest outcome before `zcode login`.
            let path = std::env::temp_dir().to_string_lossy().into_owned();
            let workspace = json!({ "workspacePath": path, "workspaceKey": path });
            match connection
                .request(
                    "session/create",
                    json!({ "workspace": workspace, "persistence":"deferred" }),
                    None,
                )
                .await
            {
                Ok(created) => {
                    let session_id = session_id_from_snapshot(&created).ok_or_else(|| {
                        DriverError::Protocol(
                            "zcode readiness session/create response is missing sessionId".into(),
                        )
                    })?;
                    let closed = connection
                        .request(
                            "session/close",
                            json!({"sessionId":session_id,"expectedPersistence":"deferred"}),
                            None,
                        )
                        .await?;
                    if closed["closed"] != true {
                        return Err(DriverError::Protocol(
                            "ZCode did not confirm closing the readiness session".into(),
                        ));
                    }
                    let after_close = connection.request("session/list", json!({}), None).await?;
                    let sessions = after_close
                        .get("sessions")
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            DriverError::Protocol(
                                "zcode follow-up session/list response is invalid".into(),
                            )
                        })?;
                    let still_listed = sessions.iter().any(|session| {
                        session.get("sessionId").and_then(Value::as_str) == Some(session_id)
                    });
                    if still_listed {
                        return Err(DriverError::Protocol(
                            "zcode readiness probe did not close its temporary session".into(),
                        ));
                    }
                    if models::catalog(&created).models.is_empty() {
                        return Err(DriverError::NotConfigured(
                            "ZCode has no available model; check its native provider settings"
                                .into(),
                        ));
                    }
                    Ok(())
                }
                Err(DriverError::NotConfigured(reason)) => Err(DriverError::NotConfigured(reason)),
                Err(error) => Err(error),
            }
        }
        .await;
        if let Err(error) = result {
            // A failed installation probe must not hand an unretired worker
            // to the next attempt. Cancellation still uses Connection's owned
            // supervisor; no native command or model task is replayed.
            connection.native.close().await?;
            return Err(error);
        }
        Ok(connection)
    }

    async fn resume_with_selection(
        &self,
        id: &str,
        workspace: &str,
        history: Option<&driver_api::HistoryMcp>,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(
            Some(id),
            SessionRequest {
                working_directory: workspace.into(),
                history_mcp: history.cloned(),
                permission_mode: None,
                additional_directories: vec![],
            },
            model,
            thinking,
        )
        .await
    }

    async fn ensure_connection(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<Connection>>, DriverError> {
        let mut guard = self.connection.lock().await;
        if let Some(connection) = guard.as_ref()
            && (connection.frames.is_closed() || connection.native.stopping())
        {
            connection.native.close().await?;
            *guard = None;
        }
        if guard.is_none() {
            let mut connection = Connection::spawn(&self.launch)?;
            connection.observe_owner(self.stopping.subscribe());
            *guard = Some(connection);
        }
        Ok(guard)
    }
}

#[async_trait]
impl AgentDriver for ZcodeDriver {
    fn independent_session_driver(&self) -> Result<Option<Arc<dyn AgentDriver>>, DriverError> {
        let driver = Self::new(
            self.executor.provider_id.as_str(),
            self.executor.installation_id.as_deref().unwrap_or(""),
            self.launch.clone(),
            self.native_version.clone(),
            self.resume_capability,
        )
        .map_err(|_| DriverError::Protocol("cannot recreate ZCode installation".into()))?;
        Ok(Some(Arc::new(driver)))
    }
    fn review_prompt_permission_mode(&self) -> Option<&str> {
        Some("yolo")
    }
    fn analysis_prompt_permission_mode(&self) -> Option<&str> {
        Some("plan")
    }
    async fn permission_catalog(
        &self,
        _workspace: Option<&str>,
    ) -> Result<driver_api::PermissionCatalog, DriverError> {
        Ok(driver_api::permissions::zcode())
    }

    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }

    fn driver_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn native_agent_version(&self) -> Option<&str> {
        self.native_version.as_deref()
    }

    fn supports_live_session_reuse(&self) -> bool {
        true
    }

    fn resume_capability(&self) -> SessionResumeCapability {
        self.resume_capability
    }

    async fn model_catalog(&self) -> Result<driver_api::ModelCatalog, DriverError> {
        self.model_catalog_in_workspace(None, None).await
    }
    async fn model_catalog_in_workspace(
        &self,
        _model: Option<&str>,
        workspace: Option<&str>,
    ) -> Result<driver_api::ModelCatalog, DriverError> {
        let workspace = workspace.map_or_else(std::env::temp_dir, PathBuf::from);
        metadata::read(self, workspace).await
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(None, request, model, thinking).await
    }
    async fn resume_selected_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open(Some(id), request, model, thinking).await
    }
    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        self.open(None, request, None, None).await
    }

    async fn resume_session(
        &self,
        native_session_id: &str,
        working_directory: &str,
    ) -> Result<SessionHandle, DriverError> {
        self.resume_with_selection(native_session_id, working_directory, None, None, None)
            .await
    }

    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: HubPromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        driver_api::permissions::zcode().validate(request.permission_mode.as_deref())?;
        let attachment_blocks = driver_api::attachments::zcode(&request.attachments).await?;
        let state = self
            .sessions
            .lock()
            .expect("sessions")
            .get(&handle.native_session_id)
            .cloned()
            .ok_or_else(|| {
                DriverError::SessionMissing("no live zcode session with this id".into())
            })?;
        let _admission = state.admission.lock().await;
        let (done, done_rx) = tokio::sync::watch::channel(None);
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let (interaction_tx, interaction_rx) = mpsc::unbounded_channel();
        {
            let _gate = state.close_result.lock().expect("session lifecycle");
            state.validate(handle)?;
            if state.closing.load(Ordering::Acquire) {
                return Err(DriverError::Disconnected("ZCode session is closing".into()));
            }
            if state.turn_active.swap(true, Ordering::AcqRel) {
                return Err(DriverError::CapabilityUnavailable(
                    "this zcode session already has an active turn".into(),
                ));
            }
            *state.turn_done.lock().expect("turn receipt") = Some(done_rx);
            *state.cancel_tx.lock().expect("cancel sender") = Some(cancel_tx);
            *state.interaction_tx.lock().expect("interaction sender") = Some(interaction_tx);
        }
        // Context items ride as text blocks before the user prompt.
        let mut content = String::new();
        for delivery in &request.context {
            let kind = match delivery.kind {
                ContextDeliveryKind::Handoff => "Handoff",
                ContextDeliveryKind::Reconciliation => "Reconciliation",
            };
            let body = String::from_utf8_lossy(&delivery.content);
            let header = format!(
                "[dogcodes {kind} {} sha256={}]\n",
                delivery.artifact_id, delivery.payload_sha256
            );
            content.push_str(&header);
            content.push_str(&body);
            content.push('\n');
        }
        content.push_str(&request.prompt);

        // The turn runs on its own task holding the connection lock until
        // the vendor turn-completion response returns; events stream through
        // the channel meanwhile, and the terminal is appended after.
        let self_connection = Arc::clone(&self.connection);
        let sessions = self.sessions.clone();
        let mode = request
            .permission_mode
            .as_deref()
            .unwrap_or("build")
            .to_owned();
        let session_id = handle.native_session_id.clone();
        let original_handle = handle.clone();
        let retired = self.retired.clone();
        let state_for_task = Arc::clone(&state);
        let latest_context = Arc::clone(&self.latest_context);
        let usage = Arc::clone(&self.usage);
        tokio::spawn(async move {
            let mut guard = self_connection.lock().await;
            if state_for_task.validate(&original_handle).is_err()
                || state_for_task.closing.load(Ordering::Acquire)
            {
                state_for_task.finish_turn(&original_handle, true);
                done.send_replace(Some(Ok(())));
                let _ = events_tx.send(DriverEvent::TurnInterrupted {
                    summary: "ZCode session closed before dispatch".into(),
                });
                return;
            }
            let Some(connection) = guard.as_mut() else {
                state_for_task.finish_turn(&original_handle, true);
                let _ = lifecycle::close_session(
                    self_connection.clone(),
                    sessions,
                    original_handle.clone(),
                    state_for_task.clone(),
                    retired.clone(),
                );
                done.send_replace(Some(Err(DriverError::Disconnected(
                    "ZCode original worker is unavailable".into(),
                ))));
                let _ = events_tx.send(DriverEvent::TurnFailed {
                    reason: "ZCode original worker is unavailable".into(),
                });
                return;
            };
            if state_for_task
                .native
                .as_ref()
                .is_some_and(|native| !Arc::ptr_eq(native, &connection.native))
            {
                state_for_task.finish_turn(&original_handle, true);
                let _ = lifecycle::close_session(
                    self_connection.clone(),
                    sessions,
                    original_handle.clone(),
                    state_for_task.clone(),
                    retired.clone(),
                );
                done.send_replace(Some(Err(DriverError::Disconnected(
                    "ZCode worker changed; restore the session first".into(),
                ))));
                let _ = events_tx.send(DriverEvent::TurnFailed {
                    reason: "ZCode worker changed; restore the session first".into(),
                });
                return;
            }
            if let Err(error) = connection
                .request(
                    "session/setMode",
                    json!({"sessionId":session_id,"mode":mode}),
                    None,
                )
                .await
            {
                state_for_task.finish_turn(&original_handle, true);
                let _ = lifecycle::close_session(
                    self_connection.clone(),
                    sessions,
                    original_handle.clone(),
                    state_for_task.clone(),
                    retired.clone(),
                );
                done.send_replace(Some(Err(error)));
                let _ = events_tx.send(DriverEvent::TurnFailed {
                    reason: "ZCode session mode was not confirmed".into(),
                });
                return;
            }
            // Capture native IDs before sending the prompt. A failed debug read
            // leaves the stable message projection as the fallback for this turn.
            let debug_before = tokio::time::timeout(
                CONTEXT_READ_TIMEOUT,
                connection.request("session/debug", json!({ "sessionId": session_id }), None),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|debug| session_usage::debug_requests(&debug, &session_id));
            let outcome = connection
                .run_turn(
                    TurnContent {
                        text: content,
                        attachments: attachment_blocks,
                    },
                    &events_tx,
                    cancel_rx,
                    interaction_rx,
                    &state_for_task,
                    &original_handle,
                )
                .await;
            let verified = outcome.is_ok();
            let terminal = match outcome {
                Ok(result) => {
                    // Read the authoritative session projection after the
                    // terminal event. Metrics failure must not change the
                    // already completed turn outcome.
                    if let Ok(Ok(snapshot)) = tokio::time::timeout(
                        CONTEXT_READ_TIMEOUT,
                        connection.request(
                            "session/read",
                            json!({ "sessionId": session_id }),
                            None,
                        ),
                    )
                    .await
                    {
                        // Always retain message IDs in the pull snapshot. Native
                        // request facts travel on the event stream, so a debug
                        // outage cannot change the next turn's baseline identity.
                        usage.set(
                            &session_id,
                            session_usage::read(&snapshot, &state_for_task.native_catalog),
                        );
                        if let Some(before) = debug_before.filter(|report| !report.truncated)
                            && let Ok(Ok(debug)) = tokio::time::timeout(
                                CONTEXT_READ_TIMEOUT,
                                connection.request(
                                    "session/debug",
                                    json!({ "sessionId": session_id }),
                                    None,
                                ),
                            )
                            .await
                            && let Some(after) = session_usage::debug_requests(&debug, &session_id)
                        {
                            for request in after.records {
                                if !before
                                    .records
                                    .iter()
                                    .any(|old| old.request_id == request.request_id)
                                {
                                    let _ = events_tx.send(DriverEvent::UsageRecorded { request });
                                }
                            }
                        }
                        if let Some(context) =
                            context_from_snapshot(&snapshot, &state_for_task.native_catalog)
                        {
                            latest_context
                                .lock()
                                .expect("context")
                                .insert(session_id.clone(), context);
                        }
                    }
                    result.terminal
                }
                Err(DriverError::QuotaExhausted { resets_at_unix }) => {
                    DriverEvent::TurnQuotaExhausted { resets_at_unix }
                }
                Err(DriverError::NotConfigured(reason)) => DriverEvent::TurnFailed { reason },
                Err(_) => DriverEvent::TurnFailed {
                    reason: "ZCode turn ended with an error result".into(),
                },
            };
            state_for_task.finish_turn(&original_handle, !verified);
            if !verified {
                let _ = lifecycle::close_session(
                    Arc::clone(&self_connection),
                    sessions,
                    original_handle.clone(),
                    state_for_task.clone(),
                    retired.clone(),
                );
            }
            done.send_replace(Some(if verified {
                Ok(())
            } else {
                Err(DriverError::Disconnected(
                    "ZCode turn did not confirm native termination".into(),
                ))
            }));
            // Publish the terminal only after the state transition. A caller
            // that immediately invokes shutdown must take the normal close
            // path instead of racing the active-turn branch.
            let _ = events_tx.send(terminal);
        });
        let stream: DriverEventStream =
            Box::pin(UnboundedReceiverStream::new(events_rx).map(Ok::<_, DriverError>));
        Ok(stream)
    }

    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        let state = self
            .sessions
            .lock()
            .expect("sessions")
            .get(&handle.native_session_id)
            .cloned()
            .ok_or_else(|| DriverError::SessionMissing("no live zcode session".into()))?;
        let outcome = {
            let _gate = state.close_result.lock().expect("session lifecycle");
            state.validate(handle)?;
            let active = state.turn_active.load(Ordering::Acquire);
            let outcome = state.turn_done.lock().expect("turn receipt").clone();
            if !active && outcome.is_none() {
                return Ok(());
            }
            if active
                && !state.interrupt_requested.swap(true, Ordering::AcqRel)
                && let Some(cancel) = state.cancel_tx.lock().expect("cancel sender").as_ref()
            {
                cancel.send(()).map_err(|_| {
                    DriverError::Disconnected("ZCode cancellation channel closed".into())
                })?;
            }
            // A failed turn can already have left the loop while its native
            // close is still pending. Retrying stop must retain that failure.
            outcome.ok_or_else(|| {
                DriverError::Disconnected("ZCode turn has no termination receipt".into())
            })?
        };
        lifecycle::wait(outcome, std::time::Duration::from_secs(10)).await
    }

    async fn respond_to_question(
        &self,
        handle: &SessionHandle,
        request_id: &str,
        answer: agent_domain::QuestionAnswer,
    ) -> Result<(), DriverError> {
        let state = self
            .sessions
            .lock()
            .expect("sessions")
            .get(&handle.native_session_id)
            .cloned()
            .ok_or_else(|| DriverError::SessionMissing("ZCode closed".into()))?;
        let _lifecycle = state.close_result.lock().expect("session lifecycle");
        state.validate(handle)?;
        if !state.accepts(handle) {
            return Err(DriverError::Disconnected(
                "ZCode interaction is no longer active".into(),
            ));
        }
        let pending = {
            let mut questions = state.pending_approvals.lock().expect("pending");
            let p = questions
                .get(request_id)
                .ok_or_else(|| DriverError::Protocol("Question expired".into()))?;
            let params = p
                .responses
                .get("question")
                .ok_or_else(|| DriverError::Protocol("Not a question".into()))?;
            driver_api::questions::native_form("zcode", params)
                .and_then(|form| form.validate(&answer))
                .map_err(DriverError::Protocol)?;
            questions.remove(request_id).expect("pending question")
        };
        let response =
            driver_api::questions::native_answer("zcode", &pending.responses["question"], &answer);
        state
            .interaction_tx
            .lock()
            .expect("sender")
            .as_ref()
            .ok_or_else(|| DriverError::Disconnected("ZCode turn ended".into()))?
            .send(InteractionDecision {
                server_id: pending.server_id,
                response,
            })
            .map_err(|_| DriverError::Disconnected("ZCode closed".into()))
    }

    async fn respond_to_interaction(
        &self,
        handle: &SessionHandle,
        native_request_id: &str,
        option_id: &str,
    ) -> Result<(), DriverError> {
        let state = self
            .sessions
            .lock()
            .expect("sessions")
            .get(&handle.native_session_id)
            .cloned()
            .ok_or_else(|| DriverError::SessionMissing("no live zcode session".into()))?;
        let _lifecycle = state.close_result.lock().expect("session lifecycle");
        state.validate(handle)?;
        if !state.accepts(handle) {
            return Err(DriverError::Disconnected(
                "ZCode interaction is no longer active".into(),
            ));
        }
        let pending = state
            .pending_approvals
            .lock()
            .expect("pending approvals")
            .remove(native_request_id)
            .ok_or_else(|| {
                DriverError::CapabilityUnavailable(
                    "this approval request is no longer pending".into(),
                )
            })?;
        if !pending
            .option_ids
            .iter()
            .any(|offered| offered == option_id)
        {
            state
                .pending_approvals
                .lock()
                .expect("pending approvals")
                .insert(native_request_id.to_owned(), pending);
            return Err(DriverError::CapabilityUnavailable(
                "unknown approval option id".into(),
            ));
        }
        let response = pending.responses.get(option_id).cloned().ok_or_else(|| {
            DriverError::Protocol("ZCode approval option omitted its native response".into())
        })?;
        let decision = InteractionDecision {
            server_id: pending.server_id,
            response,
        };
        let result = state
            .interaction_tx
            .lock()
            .expect("interaction sender")
            .as_ref()
            .ok_or_else(|| {
                DriverError::CapabilityUnavailable("this zcode turn is no longer active".into())
            })?
            .send(decision)
            .map_err(|_| DriverError::Disconnected("zcode interaction channel is closed".into()));
        if result.is_err() {
            // The native request can no longer be answered, so retaining it
            // would create a misleading retry surface.
            state
                .pending_approvals
                .lock()
                .expect("pending approvals")
                .remove(native_request_id);
        }
        result
    }

    async fn current_session_handle(&self, id: &str) -> Option<SessionHandle> {
        self.sessions
            .lock()
            .expect("sessions")
            .get(id)
            .map(|state| state.handle())
    }
    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        if let Some(state) = self
            .sessions
            .lock()
            .expect("sessions")
            .get(&handle.native_session_id)
            .cloned()
        {
            state.validate(handle).ok()?;
            return Some(Box::pin(async move { state.confirmed().await }));
        }
        self.retired
            .lock()
            .expect("retired scopes")
            .get(&handle.native_session_id)
            .filter(|old| old.same_scope(handle))
            .map(|_| Box::pin(async {}) as driver_api::SessionShutdownProof)
    }
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        let state = self
            .sessions
            .lock()
            .expect("sessions")
            .get(&handle.native_session_id)
            .cloned();
        let Some(state) = state else {
            return if self
                .retired
                .lock()
                .expect("retired scopes")
                .get(&handle.native_session_id)
                .is_some_and(|old| old.same_scope(&handle))
            {
                Ok(())
            } else {
                Err(scopes::stale())
            };
        };
        let receipt = lifecycle::close_session(
            self.connection.clone(),
            self.sessions.clone(),
            handle,
            state,
            self.retired.clone(),
        )?;
        lifecycle::wait(receipt, std::time::Duration::from_secs(10)).await
    }
}

const ZCODE_USAGE_TTL_MS: i64 = 30_000;

#[async_trait]
impl QuotaProvider for ZcodeDriver {
    async fn read_session_usage(
        &self,
        id: &str,
    ) -> Option<driver_api::usage::SessionUsageSnapshot> {
        self.usage.get(Some(id))
    }

    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }

    async fn read_quota(
        &self,
        native_session_id: Option<&str>,
    ) -> Result<QuotaSnapshot, DriverError> {
        self.read_quota_with_refresh(native_session_id, false).await
    }

    async fn read_quota_with_refresh(
        &self,
        native_session_id: Option<&str>,
        force: bool,
    ) -> Result<QuotaSnapshot, DriverError> {
        // Prefer the authorized native credential-backed monitor endpoint;
        // retain task-scoped context and fall back to context-only if unavailable.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        if let Some(mut snapshot) = self
            .quota_cache
            .lock()
            .await
            .read(&self.launch, now, force)
            .await
        {
            snapshot.context = native_session_id.and_then(|id| {
                self.latest_context
                    .lock()
                    .expect("context")
                    .get(id)
                    .cloned()
            });
            return Ok(snapshot);
        }
        if (force || native_session_id.is_none())
            && let Some(reason) = self.quota_cache.lock().await.failure
        {
            return Err(DriverError::CapabilityUnavailable(reason.into()));
        }
        let fresh = self
            .usage_pulled_at_unix_ms
            .lock()
            .expect("usage pull time")
            .is_some_and(|pulled_at| now - pulled_at < ZCODE_USAGE_TTL_MS);
        if force || !fresh {
            let mut guard = self.connection.lock().await;
            if guard.is_none() {
                let mut connection = Connection::spawn(&self.launch)?;
                connection.observe_owner(self.stopping.subscribe());
                *guard = Some(connection);
            }
            let connection = guard.as_mut().expect("connection present");
            // The pull proves the surface works; the summary itself has no
            // subscription-window semantics, so it is not surfaced as quota.
            connection
                .request("usage/stats", json!({ "range": "7d" }), None)
                .await?;
            drop(guard);
            *self
                .usage_pulled_at_unix_ms
                .lock()
                .expect("usage pull time") = Some(now);
        }
        let context = native_session_id.and_then(|session_id| {
            self.latest_context
                .lock()
                .expect("context")
                .get(session_id)
                .cloned()
        });
        Ok(QuotaSnapshot {
            session_usage: None,
            windows: vec![],
            context,
            collected_at_unix_ms: now,
            stale_after_unix_ms: now + ZCODE_USAGE_TTL_MS,
            plan_type: Some("额度不可用：请检查 ZCode 当前模型与登录状态".into()),
            source: "native_pull".into(),
            confidence: "derived".into(),
            parser_version: "zcode-usage-stats-v1".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn readiness_fixture(with_model: bool) -> (tempfile::TempDir, ZcodeLaunch) {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        let models = if with_model {
            json!([{"ref":{"providerId":"fixture","modelId":"fixture"}}])
        } else {
            json!([])
        };
        let created = json!({"sessionId":"probe", "settings":{"model":{"available":models}}});
        let real = json!({"sessionId":"real","session":{"sessionId":"real","workspace":{"workspacePath":root.path()},"status":"idle"}});
        let script = format!(
            "#!/bin/sh\ni=0\nwhile IFS= read -r request; do\n  i=$((i+1))\n  case $i in\n    1|4) result='{{\"sessions\":[]}}';;\n    2) result='{created}';;\n    5) result='{real}';;\n    *) result='{{}}';;\n  esac\n  printf '{{\"id\":%s,\"result\":%s}}\\n' \"$i\" \"$result\"\ndone\n"
        );
        let script = script.replace(
            "    *) result='{}';;",
            r#"    3|7) result='{"closed":true}';;
    *) result='{}';;"#,
        );
        std::fs::write(&runtime, script).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        let launch = ZcodeLaunch {
            app_binary: runtime,
            cli_script: root.path().join("unused.cjs"),
        };
        (root, launch)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ready_driver_reuses_process_and_recovers_if_it_exits() {
        let (root, launch) = readiness_fixture(true);
        let driver = ZcodeDriver::connect_ready(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch,
            None,
            SessionResumeCapability::Unknown,
        )
        .await
        .unwrap();
        let pid = driver.connection.lock().await.as_ref().unwrap().native.id();
        let handle = driver
            .create_session(SessionRequest {
                working_directory: root.path().to_string_lossy().into_owned(),
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
            })
            .await
            .unwrap();
        // The fifth response can only come from the retained process, after
        // list/create/close/list have removed the readiness session.
        assert_eq!(handle.native_session_id, "real");
        assert!(
            !driver
                .sessions
                .lock()
                .expect("sessions")
                .contains_key("probe")
        );
        driver.shutdown(handle).await.unwrap();
        {
            let mut guard = driver.connection.lock().await;
            let connection = guard.as_mut().unwrap();
            assert_eq!(connection.native.id(), pid);
            connection.native.close().await.unwrap();
        }
        let mut guard = driver.ensure_connection().await.unwrap();
        let connection = guard.as_mut().unwrap();
        assert_ne!(connection.native.id(), pid);
        assert_eq!(
            connection
                .request("session/list", json!({}), None)
                .await
                .unwrap(),
            json!({"sessions":[]})
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_readiness_still_rejects_missing_models() {
        let (_root, launch) = readiness_fixture(false);
        let result = ZcodeDriver::connect_ready(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch,
            None,
            SessionResumeCapability::Unknown,
        )
        .await;
        assert!(matches!(result, Err(DriverError::NotConfigured(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_server_receives_relocated_native_config_paths() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        std::fs::write(&runtime, concat!(
            "#!/bin/sh\n",
            "read request\n",
            "if [ -f \"$ZCODE_BUILTIN_PROVIDER_CONFIG_FILE\" ] && [ \"$ZCODE_PERSONAL_PROVIDER_CONFIG_FILE\" = \"$HOME/.zcode/v2/provider_config.json\" ]; then\n",
            "  echo '{\"id\":1,\"result\":{\"sessions\":[]}}'\n",
            "fi\n"
        )).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        let launch = ZcodeLaunch {
            app_binary: runtime,
            cli_script: root.path().join("Resources/glm/zcode.cjs"),
        };
        assert!(launch.bundled_provider_config().is_none());
        let config = root
            .path()
            .join("Resources/config/provider/zcode-builtin.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "{}").unwrap();
        let mut connection = Connection::spawn(&launch).unwrap();
        let result = connection
            .request("session/list", json!({}), None)
            .await
            .unwrap();
        assert_eq!(result, json!({"sessions":[]}));
    }

    #[test]
    fn parses_the_verified_wire_shapes() {
        let response = parse_frame(r#"{"id":1,"result":{"sessions":[]}}"#).unwrap();
        assert!(matches!(response, Incoming::Response { .. }));
        let server_request = parse_frame(
            r#"{"id":"server-1","method":"session/requestRuntimePreferences","params":{"sessionId":"s"}}"#,
        )
        .unwrap();
        match server_request {
            Incoming::ServerRequest { method, .. } => {
                assert_eq!(method, "session/requestRuntimePreferences");
            }
            other => panic!("unexpected: {other:?}"),
        }
        let notification = parse_frame(
            r#"{"method":"session/event","params":{"payload":{"turnNumber":0,"input":"hi","messageId":"m1","turnId":"t1"}}}"#,
        )
        .unwrap();
        match notification {
            Incoming::Notification { method, params } => {
                assert_eq!(method, "session/event");
                let event = map_notification(&method, &params).unwrap();
                assert!(matches!(
                    event,
                    DriverEvent::TurnStarted { native_turn_id: Some(ref id) } if id == "t1"
                ));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn authoritative_event_type_prevents_tool_result_from_becoming_turn_terminal() {
        let params = json!({
            "type": "tool.updated",
            "payload": {
                "kind": "result",
                "resultType": "text",
                "toolCallId": "tool-1"
            }
        });
        assert_eq!(event_method("session/event", &params), "tool.updated");
    }

    #[test]
    fn zcode_1308_maps_to_structured_quota_exhaustion() {
        assert!(matches!(
            stable_turn_failure(&json!({
                "error": { "code": 1308, "next_flush_time": 1_800_000_000 }
            })),
            DriverEvent::TurnQuotaExhausted {
                resets_at_unix: Some(1_800_000_000)
            }
        ));
    }

    #[test]
    fn model_config_missing_maps_to_not_configured() {
        let error = json!({
            "code": -32603,
            "message": "ModelProtocolError: Model config is missing. Create ~/.zcode/cli/config.json with an explicit model provider before running ZCode."
        });
        assert!(matches!(
            map_request_error("session/create", &error),
            DriverError::NotConfigured(_)
        ));
    }

    #[test]
    fn approvals_are_declined_and_streaming_maps_text() {
        assert_eq!(
            answer_server_request("interaction/requestPermission", &json!({}), None),
            json!({ "decision": "deny" })
        );
        let event = map_notification(
            "session/event",
            &json!({
                "sessionId": "s1",
                "payload": { "kind": "text_delta", "delta": "hello" }
            }),
        )
        .unwrap();
        assert!(matches!(
            event,
            DriverEvent::TextDelta { ref text, .. } if text == "hello"
        ));
        assert!(
            map_notification(
                "session/event",
                &json!({ "payload": { "kind": "reasoning_delta", "delta": "hidden" } }),
            )
            .is_none()
        );
    }

    #[test]
    fn permission_options_are_preserved_with_semantic_roles() {
        let (request_id, event, option_ids) = permission_request_event(&json!({
            "requestId": "request-1",
            "toolName": "bash",
            "input": { "command": "printf safe" },
            "options": [
                { "kind": "allow_once", "name": "Allow once", "optionId": "allow_once" },
                { "kind": "allow_always", "name": "Always allow", "optionId": "allow_project" },
                { "kind": "deny", "name": "Deny", "optionId": "deny" }
            ]
        }))
        .expect("verified permission shape");
        assert_eq!(request_id, "request-1");
        assert_eq!(option_ids, ["allow_once", "allow_project", "deny"]);
        let DriverEvent::ApprovalRequested {
            tool_name, options, ..
        } = event
        else {
            panic!("expected approval event");
        };
        assert_eq!(tool_name, "bash");
        assert_eq!(options[0].semantic_role, ApprovalSemanticRole::ApproveOnce);
        assert_eq!(
            options[1].semantic_role,
            ApprovalSemanticRole::ApprovePersistent
        );
        assert_eq!(options[2].semantic_role, ApprovalSemanticRole::Reject);
    }

    #[tokio::test]
    async fn live_session_reuse_does_not_require_verified_cold_resume() {
        let driver = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            ZcodeLaunch {
                app_binary: "/unused/zcode".into(),
                cli_script: "/unused/zcode.cjs".into(),
            },
            Some("0.16.9".into()),
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        driver.sessions.lock().expect("sessions").insert(
            "live".into(),
            Arc::new(SessionState {
                scope: std::sync::Mutex::new(SessionHandle::scoped("live".into())),
                workspace: "/tmp".into(),
                admission: tokio::sync::Mutex::new(()),
                retired: tokio::sync::watch::channel(false).0,
                native_catalog: Value::Null,
                turn_active: AtomicBool::new(false),
                interrupt_requested: AtomicBool::new(false),
                cancel_tx: std::sync::Mutex::new(None),
                interaction_tx: std::sync::Mutex::new(None),
                pending_approvals: std::sync::Mutex::new(HashMap::new()),
                closing: AtomicBool::new(false),
                close_result: std::sync::Mutex::new(None),
                turn_done: std::sync::Mutex::new(None),
                native: None,
            }),
        );
        assert_eq!(
            driver
                .resume_session("live", "/tmp")
                .await
                .unwrap()
                .native_session_id,
            "live"
        );
        assert!(matches!(
            driver.resume_session("not-live", "/tmp").await,
            Err(DriverError::CapabilityUnavailable(_))
        ));
        assert!(driver.connection.lock().await.is_none());
    }

    #[tokio::test]
    async fn interaction_response_accepts_only_a_vendor_option() {
        let driver = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            ZcodeLaunch {
                app_binary: PathBuf::from("/unused/zcode"),
                cli_script: PathBuf::from("/unused/zcode.cjs"),
            },
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let (interaction_tx, mut interaction_rx) = mpsc::unbounded_channel();
        let state = Arc::new(SessionState {
            scope: std::sync::Mutex::new(SessionHandle::scoped("session-1".into())),
            workspace: "/tmp".into(),
            admission: tokio::sync::Mutex::new(()),
            retired: tokio::sync::watch::channel(false).0,
            native_catalog: Value::Null,
            turn_active: AtomicBool::new(true),
            interrupt_requested: AtomicBool::new(false),
            cancel_tx: std::sync::Mutex::new(None),
            interaction_tx: std::sync::Mutex::new(Some(interaction_tx)),
            pending_approvals: std::sync::Mutex::new(HashMap::from([(
                "request-1".into(),
                PendingApproval {
                    server_id: json!(42),
                    option_ids: vec!["allow_once".into(), "deny".into()],
                    responses: HashMap::from([
                        ("allow_once".into(), json!({"decision":"allow"})),
                        ("deny".into(), json!({"decision":"deny"})),
                    ]),
                },
            )])),
            closing: AtomicBool::new(false),
            close_result: std::sync::Mutex::new(None),
            turn_done: std::sync::Mutex::new(None),
            native: None,
        });
        driver
            .sessions
            .lock()
            .expect("sessions")
            .insert("session-1".into(), state);
        let handle = driver.current_session_handle("session-1").await.unwrap();
        assert!(
            driver
                .respond_to_interaction(&handle, "request-1", "invented")
                .await
                .is_err()
        );
        driver
            .respond_to_interaction(&handle, "request-1", "allow_once")
            .await
            .unwrap();
        let decision = interaction_rx.recv().await.unwrap();
        assert_eq!(decision.server_id, json!(42));
        assert_eq!(decision.response, json!({"decision":"allow"}));
    }

    #[test]
    fn extracts_nested_session_and_context_projection() {
        let snapshot = json!({
            "session": { "sessionId": "session-1" },
            "projection": { "contextUsed": 1_234, "contextWindow": 200_000 }
        });
        assert_eq!(session_id_from_snapshot(&snapshot), Some("session-1"));
        assert_eq!(
            context_from_snapshot(&snapshot, &Value::Null),
            Some(ContextUsageSnapshot {
                used_tokens: 1_234,
                window_tokens: 200_000,
                cache_hit_rate_percent: None,
            })
        );
    }

    #[test]
    fn maps_enveloped_tool_lifecycle_with_stable_id() {
        let started = map_notification(
            "session/event",
            &json!({
                "payload": {
                    "kind": "started",
                    "toolCallId": "call-1",
                    "toolName": "shell"
                }
            }),
        )
        .unwrap();
        assert!(matches!(
            started,
            DriverEvent::ToolStarted { ref item_id, ref tool_name, .. }
                if item_id == "call-1" && tool_name == "shell"
        ));
        let completed = map_notification(
            "session/event",
            &json!({
                "payload": {
                    "kind": "result",
                    "toolCallId": "call-1",
                    "toolName": "shell",
                    "result": {}
                }
            }),
        )
        .unwrap();
        assert!(matches!(
            completed,
            DriverEvent::ToolCompleted { ref item_id, success: true, .. }
                if item_id == "call-1"
        ));
    }
}

/// Real-installation probes; run only with `--ignored`. `probe_ready` makes no
/// model calls — it exercises `session/list` plus a `session/create` attempt
/// whose "Model config is missing" error is the honest `NotConfigured` answer
/// until the operator runs `zcode login` once.
#[cfg(test)]
mod real_probes {
    use super::*;

    fn launch() -> ZcodeLaunch {
        ZcodeLaunch {
            app_binary: PathBuf::from("/Applications/ZCode.app/Contents/MacOS/ZCode"),
            cli_script: PathBuf::from("/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires the local ZCode desktop installation; run with --ignored"]
    async fn probes_the_current_users_zcode_installation() {
        match ZcodeDriver::probe_ready(&launch()).await {
            Ok(()) => println!("zcode: app-server surface ready (login configured)"),
            Err(DriverError::NotConfigured(reason)) => {
                println!("zcode: not configured (expected before `zcode login`): {reason}");
            }
            Err(error) => panic!("zcode: probe failed unexpectedly: {error}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "runs two native model turns to verify live-session reuse"]
    async fn real_live_session_reuse_after_turn() {
        use futures::StreamExt;
        let driver = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch(),
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let cwd = workspace.path().to_string_lossy().into_owned();
        let mut session = driver
            .create_session(SessionRequest {
                working_directory: cwd.clone(),
                additional_directories: vec![],
                history_mcp: None,
                permission_mode: None,
            })
            .await
            .unwrap();
        for turn in 0..2 {
            let handle = if turn == 0 {
                session.clone()
            } else {
                driver
                    .resume_session(&session.native_session_id, &cwd)
                    .await
                    .unwrap()
            };
            let mut stream = driver
                .prompt(
                    &handle,
                    HubPromptRequest {
                        attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt: "Reply with exactly DOGCODES_OK. Do not use tools or modify files."
                            .into(),
                        working_directory: cwd.clone(),
                        context: vec![],
                    },
                )
                .await
                .unwrap();
            let (text, completed) =
                tokio::time::timeout(std::time::Duration::from_mins(2), async {
                    let mut text = String::new();
                    while let Some(event) = stream.next().await {
                        match event.unwrap() {
                            DriverEvent::TextDelta { text: delta, .. } => text.push_str(&delta),
                            DriverEvent::TurnCompleted { .. } => return (text, true),
                            DriverEvent::TurnFailed { .. }
                            | DriverEvent::TurnInterrupted { .. }
                            | DriverEvent::TurnQuotaExhausted { .. } => return (text, false),
                            _ => {}
                        }
                    }
                    (text, false)
                })
                .await
                .unwrap();
            println!(
                "zcode turn={} completed={} marker={}",
                turn + 1,
                completed,
                text.contains("DOGCODES_OK")
            );
            assert!(completed && text.contains("DOGCODES_OK"));
            session = handle;
        }
        driver.shutdown(session).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_turn_acceptance_streams_and_completes() {
        let driver = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch(),
            Some("0.16.5".into()),
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: workspace.path().to_string_lossy().into_owned(),
            })
            .await
            .expect("session/create must succeed after login");
        let stream = driver
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "Reply with exactly: OK".into(),
                    working_directory: workspace.path().to_string_lossy().into_owned(),
                    context: vec![],
                },
            )
            .await
            .expect("turn must be accepted");
        let events: Vec<DriverEvent> =
            tokio::time::timeout(std::time::Duration::from_secs(190), async {
                use futures::StreamExt;
                let mut stream = std::pin::pin!(stream);
                let mut events = Vec::new();
                while let Some(event) = stream.next().await {
                    let event = event.unwrap();
                    let terminal = matches!(
                        event,
                        DriverEvent::TurnCompleted { .. }
                            | DriverEvent::TurnFailed { .. }
                            | DriverEvent::TurnQuotaExhausted { .. }
                            | DriverEvent::TurnInterrupted { .. }
                    );
                    events.push(event);
                    if terminal {
                        break;
                    }
                }
                events
            })
            .await
            .expect("turn must finish in time");
        let quota = driver
            .read_quota(Some(&session.native_session_id))
            .await
            .expect("session context quota read must succeed");
        driver.shutdown(session).await.unwrap();
        let text = events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        println!("zcode turn events: {} (text: {text:?})", events.len());
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "turn must complete: {events:?}"
        );
        assert!(
            !text.is_empty(),
            "real response must stream text: {events:?}"
        );
        let context = quota
            .context
            .expect("completed real turn must publish session context usage");
        assert!(context.window_tokens > 0);
        assert!(context.used_tokens > 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_turn_cancellation_reaches_interrupted_terminal() {
        let driver = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch(),
            Some("0.16.5".into()),
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: workspace.path().to_string_lossy().into_owned(),
            })
            .await
            .expect("session/create must succeed after login");
        let mut stream = driver
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "Write a detailed 2,000 word explanation of Rust ownership.".into(),
                    working_directory: workspace.path().to_string_lossy().into_owned(),
                    context: vec![],
                },
            )
            .await
            .expect("turn must be accepted");
        let mut events = Vec::new();
        let started = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                let started = matches!(event, DriverEvent::TurnStarted { .. });
                let terminal = matches!(
                    event,
                    DriverEvent::TurnCompleted { .. }
                        | DriverEvent::TurnFailed { .. }
                        | DriverEvent::TurnQuotaExhausted { .. }
                        | DriverEvent::TurnInterrupted { .. }
                );
                events.push(event);
                if started || terminal {
                    return started;
                }
            }
            false
        })
        .await
        .expect("turn must start in time");
        if !started {
            driver.shutdown(session).await.unwrap();
            panic!("turn ended before cancellation could be tested: {events:?}");
        }
        driver
            .interrupt(&session)
            .await
            .expect("stop must be accepted");
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                let terminal = matches!(
                    event,
                    DriverEvent::TurnCompleted { .. }
                        | DriverEvent::TurnFailed { .. }
                        | DriverEvent::TurnQuotaExhausted { .. }
                        | DriverEvent::TurnInterrupted { .. }
                );
                events.push(event);
                if terminal {
                    break;
                }
            }
        })
        .await
        .expect("cancelled turn must reach terminal in time");
        driver.shutdown(session).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnInterrupted { .. })),
            "cancel must produce TurnInterrupted: {events:?}"
        );
    }

    fn exact_fixture_command(command: &str, target: &std::path::Path) -> bool {
        let target = target.to_string_lossy();
        let commands = [
            format!("printf DOGCODES_APPROVAL_OK > {target}"),
            format!("printf 'DOGCODES_APPROVAL_OK' > {target}"),
            format!("printf '%s' 'DOGCODES_APPROVAL_OK' > {target}"),
            format!("printf %s DOGCODES_APPROVAL_OK > {target}"),
        ];
        commands.iter().any(|expected| command.trim() == expected)
    }

    fn safe_fixture_input(
        input: &Value,
        workspace: &std::path::Path,
        target: &std::path::Path,
    ) -> bool {
        let cwd_is_safe = input
            .get("cwd")
            .or_else(|| input.get("workingDirectory"))
            .and_then(Value::as_str)
            .is_none_or(|cwd| cwd == workspace.to_string_lossy());
        let command_is_safe = input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(Value::as_str)
            .is_some_and(|command| exact_fixture_command(command, target));
        cwd_is_safe && command_is_safe
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes two real model turns; run only with explicit operator authorization"]
    async fn real_approval_deny_then_allow_only_in_temporary_workspace() {
        let cases: Vec<bool> = match std::env::var("DOGCODES_APPROVAL_CASE").as_deref() {
            Ok("deny") => vec![false],
            Ok("allow") => vec![true],
            _ => vec![false, true],
        };
        for allow in cases {
            let workspace = tempfile::tempdir().unwrap();
            let cwd = workspace.path().to_string_lossy().into_owned();
            let target = workspace.path().join("approval.txt");
            let fixture_command =
                format!("printf DOGCODES_APPROVAL_OK > {}", target.to_string_lossy());
            let driver = ZcodeDriver::new(
                "zcode",
                &agent_domain::InstallationId::new().to_string(),
                launch(),
                Some("0.16.5".into()),
                SessionResumeCapability::Supported,
            )
            .unwrap();
            let session = driver
                .create_session(SessionRequest {
                    history_mcp: None,
                    permission_mode: None,
                    additional_directories: vec![],
                    working_directory: cwd.clone(),
                })
                .await
                .unwrap();
            let mut stream = driver
                .prompt(
                    &session,
                    HubPromptRequest {
                                                attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt: format!(
                            "Approval integration test. Use a shell tool to run exactly: {fixture_command} . Request approval. Do not use other tools or access other paths. If denied, stop and report denied."
                        ),
                        working_directory: cwd,
                        context: vec![],
                    },
                )
                .await
                .unwrap();
            let outcome = tokio::time::timeout(std::time::Duration::from_mins(2), async {
                let mut approvals = 0;
                let mut allowed = 0;
                let mut completed = false;
                let mut terminal = false;
                let mut tool_started = 0;
                let mut tool_succeeded = 0;
                let mut tool_failed = 0;
                let mut terminal_failure = None;
                while let Some(event) = stream.next().await {
                    match event.unwrap() {
                        DriverEvent::ApprovalRequested {
                            native_request_id,
                            tool_name,
                            command_summary,
                            options,
                        } => {
                            approvals += 1;
                            let input = serde_json::from_str::<Value>(&command_summary)
                                .unwrap_or_default();
                            let safe = matches!(
                                tool_name.to_ascii_lowercase().as_str(),
                                "bash" | "shell" | "command"
                            ) && safe_fixture_input(&input, workspace.path(), &target);
                            let role = if allow && safe {
                                allowed += 1;
                                ApprovalSemanticRole::ApproveOnce
                            } else {
                                ApprovalSemanticRole::Reject
                            };
                            let option = options
                                .iter()
                                .find(|option| option.semantic_role == role)
                                .expect("ZCode did not offer the required decision");
                            if allow && !safe {
                                println!(
                                    "zcode approval fixture mismatch: tool={tool_name}, shape={}, bytes={}",
                                    if input.is_object() { "object" } else { "other" },
                                    command_summary.len()
                                );
                            }
                            driver
                                .respond_to_interaction(
                                    &session,
                                    &native_request_id,
                                    &option.option_id,
                                )
                                .await
                                .unwrap();
                        }
                        DriverEvent::TurnCompleted { .. } => {
                            completed = true;
                            terminal = true;
                        }
                        DriverEvent::TurnFailed { reason } => {
                            terminal_failure = Some(reason);
                            terminal = true;
                        }
                        DriverEvent::TurnQuotaExhausted { resets_at_unix } => {
                            terminal_failure = Some(format!(
                                "ZCode quota exhausted; resets_at_unix={resets_at_unix:?}"
                            ));
                            terminal = true;
                        }
                        DriverEvent::TurnInterrupted { summary } => {
                            terminal_failure = Some(summary);
                            terminal = true;
                        }
                        DriverEvent::TextDelta { text, .. } => println!("zcode fixture response: {text}"),
                        DriverEvent::ToolStarted { .. } => tool_started += 1,
                        DriverEvent::ToolCompleted { success: true, .. } => tool_succeeded += 1,
                        DriverEvent::ToolCompleted { success: false, .. } => tool_failed += 1,
                        _ => {}
                    }
                }
                (
                    approvals,
                    allowed,
                    completed,
                    terminal,
                    tool_started,
                    tool_succeeded,
                    tool_failed,
                    terminal_failure,
                )
            })
            .await;
            driver.shutdown(session).await.unwrap();
            let (
                approvals,
                allowed,
                completed,
                terminal,
                tool_started,
                tool_succeeded,
                tool_failed,
                terminal_failure,
            ) = outcome.expect("ZCode approval turn timed out");
            println!(
                "zcode: allow={allow}, approvals={approvals}, allowed={allowed}, completed={completed}, tool_started={tool_started}, tool_succeeded={tool_succeeded}, tool_failed={tool_failed}, marker_written={}, terminal_failure={terminal_failure:?}",
                target.exists(),
            );
            assert!(
                approvals > 0,
                "ZCode did not request approval; terminal_failure={terminal_failure:?}"
            );
            assert!(terminal, "ZCode approval turn had no terminal");
            if allow {
                assert_eq!(allowed, 1, "safe allow was not sent to ZCode");
                assert!(completed, "allowed ZCode turn did not complete");
                assert_eq!(
                    std::fs::read_to_string(target).unwrap(),
                    "DOGCODES_APPROVAL_OK"
                );
            } else {
                assert!(!target.exists(), "denied ZCode tool wrote the file");
            }
            println!("zcode: allow={allow}, approval and filesystem outcome verified");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes two real model turns; run only with explicit operator authorization"]
    async fn real_resume_reattaches_across_driver_instances() {
        let workspace = tempfile::tempdir().unwrap();
        let cwd = workspace.path().to_string_lossy().into_owned();
        let first = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch(),
            Some("0.16.5".into()),
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let session = first
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: cwd.clone(),
            })
            .await
            .unwrap();
        let native_session_id = session.native_session_id.clone();
        let first_events: Vec<DriverEvent> = first
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "Remember the word: dogcodes. Reply with exactly: remembered".into(),
                    working_directory: cwd.clone(),
                    context: vec![],
                },
            )
            .await
            .unwrap()
            .map(|event| event.unwrap())
            .collect()
            .await;
        assert!(
            first_events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
        first.shutdown(session).await.unwrap();
        drop(first);

        let second = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch(),
            Some("0.16.5".into()),
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let resumed = second
            .resume_session(&native_session_id, &cwd)
            .await
            .expect("fresh driver must resume the native session");
        assert_eq!(resumed.native_session_id, native_session_id);
        let second_events: Vec<DriverEvent> = second
            .prompt(
                &resumed,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "What word did I ask you to remember? Reply with exactly that word."
                        .into(),
                    working_directory: cwd,
                    context: vec![],
                },
            )
            .await
            .unwrap()
            .map(|event| event.unwrap())
            .collect()
            .await;
        let text = second_events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::TextDelta { text, .. } => Some(text.to_ascii_lowercase()),
                _ => None,
            })
            .collect::<String>();
        assert!(
            second_events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
        assert!(
            text.contains("dogcodes"),
            "resume did not restore context: {text}"
        );
        second.shutdown(resumed).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "may attempt one real model turn; run only with explicit operator authorization"]
    async fn real_resume_control_plane_reattaches_after_terminal() {
        let workspace = tempfile::tempdir().unwrap();
        let cwd = workspace.path().to_string_lossy().into_owned();
        let first = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch(),
            Some("0.16.5".into()),
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let session = first
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: cwd.clone(),
            })
            .await
            .unwrap();
        let native_session_id = session.native_session_id.clone();
        let _: Vec<DriverEvent> = first
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "Reply with exactly: OK".into(),
                    working_directory: cwd.clone(),
                    context: vec![],
                },
            )
            .await
            .unwrap()
            .map(|event| event.unwrap())
            .collect()
            .await;
        first.shutdown(session).await.unwrap();
        drop(first);

        let second = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch(),
            Some("0.16.5".into()),
            SessionResumeCapability::Supported,
        )
        .unwrap();
        let resumed = second
            .resume_session(&native_session_id, &cwd)
            .await
            .expect("fresh process must reattach after a terminal turn");
        assert_eq!(resumed.native_session_id, native_session_id);
        second.shutdown(resumed).await.unwrap();
    }
}

/// Real quota probe: `usage/stats` must answer after the config exists.
#[cfg(test)]
mod real_quota_probe {
    use super::*;
    use driver_api::QuotaProvider;

    fn launch() -> ZcodeLaunch {
        ZcodeLaunch {
            app_binary: PathBuf::from("/Applications/ZCode.app/Contents/MacOS/ZCode"),
            cli_script: PathBuf::from("/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires the local ZCode installation with a valid model config"]
    async fn usage_stats_answers_quota_reads() {
        let driver = ZcodeDriver::new(
            "zcode",
            &agent_domain::InstallationId::new().to_string(),
            launch(),
            Some("0.16.5".into()),
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let snapshot = driver
            .read_quota(None)
            .await
            .expect("usage/stats pull must answer");
        println!(
            "zcode quota: windows={} context={:?} source={}",
            snapshot.windows.len(),
            snapshot
                .context
                .as_ref()
                .map(|c| (c.used_tokens, c.window_tokens)),
            snapshot.source,
        );
    }
}

#[cfg(test)]
mod cold_resume_errors {
    use super::*;
    #[test]
    fn only_explicit_native_missing_errors_are_session_missing() {
        for error in [
            json!({"code":-32000,"message":"Session not found: private-id"}),
            json!({"code":-32000,"message":"private","data":{"code":"session_not_found"}}),
        ] {
            let mapped = map_request_error("session/resume", &error);
            assert!(matches!(mapped, DriverError::SessionMissing(_)));
            assert!(!mapped.to_string().contains("private"));
        }
        for message in ["request timed out", "session_corrupted", "turn_in_progress"] {
            assert!(matches!(
                map_request_error("session/resume", &json!({"message":message})),
                DriverError::Protocol(_)
            ));
        }
    }
    #[test]
    fn unverified_native_versions_are_not_promoted() {
        assert_eq!(
            verified_resume_capability(Some("0.16.9")),
            SessionResumeCapability::Supported
        );
        assert_eq!(
            verified_resume_capability(Some("0.16.10")),
            SessionResumeCapability::Unknown
        );
        assert_eq!(
            verified_resume_capability(Some("0.16.5")),
            SessionResumeCapability::Unknown
        );
        assert_eq!(
            verified_resume_capability(Some("0.16.6")),
            SessionResumeCapability::Unknown
        );
        assert_eq!(
            verified_resume_capability(None),
            SessionResumeCapability::Unknown
        );
    }
    #[test]
    fn unavailable_runtime_preserves_category_without_vendor_payload() {
        let error = map_request_error(
            "session/send",
            &json!({"code":-32031,"message":"private vendor text","data":{"code":"ZCODE_RUNTIME_MODEL_UNAVAILABLE","sessionId":"private-session"}}),
        );
        assert!(matches!(error, DriverError::NotConfigured(_)));
        assert!(!error.to_string().contains("private"));
    }
}
