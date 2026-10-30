//! ACP stable-v1 product driver.
//!
//! Connects to one user-installed agent that exposes an Agent Client Protocol
//! server over stdio (native ACP mode, or an ecosystem adapter such as
//! `claude-agent-acp`). The driver pins the official `agent-client-protocol`
//! SDK at stable v1 with no unstable features and maps the ACP session stream
//! onto the Hub's [`DriverEvent`] model.
//!
//! Permission requests remain pending until the operator chooses one of the
//! native options, or cancellation closes the request. Native session modes
//! are discovered independently of models and applied before prompt dispatch.

mod auxiliary;
#[cfg(all(test, unix))]
mod cleanup_tests;
mod codebuddy_question;
mod grok_usage;
mod lifecycle;
mod metadata;
#[cfg(all(test, unix))]
mod metadata_tests;
mod opencode_models;
#[cfg(test)]
mod recovery_tests;
mod request_usage;
mod session_usage;
mod sessions;
mod turn;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol::schema::ProtocolVersion;
#[allow(unused_imports)]
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AgentResponse, CancelNotification, ClientRequest, CloseSessionRequest,
    CloseSessionResponse, ContentBlock, ContentChunk, ExtRequest, ExtResponse, InitializeRequest,
    InitializeResponse, LoadSessionRequest, NewSessionRequest, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionId, SessionNotification,
    SessionUpdate, StopReason, TextContent, ToolCallStatus,
};
use agent_client_protocol::{
    Agent, ByteStreams, Client, ConnectionTo, Error as AcpError, Responder,
    on_receive_notification, on_receive_request,
};
use async_trait::async_trait;
mod activity;
#[cfg(test)]
mod codebuddy_reuse_tests;
mod cursor_quota;
mod kiro_quota;
mod models;
mod permissions;
mod session_state;
use agent_domain::{ExecutorRef, ProviderId};
use driver_api::{
    AgentDriver, ApprovalOption, ApprovalSemanticRole, ContextDelivery, ContextDeliveryKind,
    ContextUsageSnapshot, DriverError, DriverEvent, DriverEventStream,
    PromptRequest as HubPromptRequest, QuotaProvider, QuotaSnapshot, QuotaWindow, SessionHandle,
    SessionRequest, SessionResumeCapability, explicitly_reports_quota_exhaustion,
};
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Environment variables handed to the agent process; everything else is
/// stripped with `env_clear` so credentials never leak into the agent.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "GROK_HOME",
    "CODEBUDDY_CONFIG_DIR",
    "DSH_HOME",
    "OPENCODE_DB",
    "XDG_DATA_HOME",
    "LANG",
    "TZ",
    "TMPDIR",
    "TERM",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];
/// Live context readings expire after five minutes without an update.
const ACP_CONTEXT_TTL_MS: i64 = 5 * 60 * 1_000;
const GROK_QUOTA_TTL_MS_I64: i64 = 30_000;
const GROK_BILLING_PARSER_VERSION: &str = "grok-x-ai-billing-v3";
const KIMI_OAUTH_USAGE_PARSER_VERSION: &str = "kimi-oauth-usage-v2";

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Use the CLI's loopback management endpoint without opening a browser.
/// No model turn is sent. All reads are bounded and credentials stay in memory.
async fn read_kimi_web_usage(
    executable: &Path,
    permit: tokio::sync::OwnedMutexGuard<()>,
) -> Result<serde_json::Value, DriverError> {
    let executable = executable.to_path_buf();
    let (mut reply, result) = oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        if reply.is_closed() {
            return;
        }
        match read_kimi_web_usage_owned(&executable, &mut reply).await {
            Ok(Some(value)) => {
                let _ = reply.send(Ok(value));
            }
            Err(error) => {
                let _ = reply.send(Err(error));
            }
            Ok(None) => {}
        }
    });
    result.await.map_err(|_| driver_error_from_channel())?
}

async fn read_kimi_web_usage_owned(
    executable: &Path,
    reply: &mut oneshot::Sender<Result<serde_json::Value, DriverError>>,
) -> Result<Option<serde_json::Value>, DriverError> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let mut command = tokio::process::Command::new(executable);
    command
        .args(["web", "--port", "0", "--no-open"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for variable in ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(variable) {
            command.env(variable, value);
        }
    }
    agent_runtime::apply_proxy_environment(&mut command);
    let mut child = agent_runtime::OwnedChild::spawn(command)
        .map_err(|_| DriverError::Disconnected("Kimi usage server could not start".into()))?;
    let stdout = child.stdout.take();
    // The temporary web helper has no stdin-driven shutdown. Terminate its
    // owned scope directly instead of waiting for an EOF it cannot observe.
    let process = lifecycle::Process::start_with_grace(child, None, std::time::Duration::ZERO);
    let query = async {
        let stdout = stdout.ok_or_else(driver_error_from_channel)?;
        // Bound the entire banner, including a malicious/malformed unterminated line.
        let mut reader = BufReader::new(stdout.take(65_536));
        let endpoint = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let mut line = String::new();
                if reader
                    .read_line(&mut line)
                    .await
                    .map_err(|_| driver_error_from_channel())?
                    == 0
                {
                    return Err(DriverError::Protocol(
                        "Kimi usage endpoint missing from bounded banner".into(),
                    ));
                }
                if let Some(offset) = line.find("http://127.0.0.1:") {
                    let url = reqwest::Url::parse(line[offset..].trim()).map_err(|_| {
                        DriverError::Protocol("Invalid Kimi loopback endpoint".into())
                    })?;
                    if url.host_str() != Some("127.0.0.1") || url.port().is_none() {
                        continue;
                    }
                    let token = url
                        .fragment()
                        .and_then(|value| value.strip_prefix("token="))
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| DriverError::Protocol("Kimi endpoint token missing".into()))?
                        .to_owned();
                    return Ok((url.port().unwrap(), token));
                }
            }
        })
        .await
        .map_err(|_| DriverError::Disconnected("Kimi usage server startup timed out".into()))??;
        fetch_kimi_usage(endpoint.0, &endpoint.1).await
    };
    let result = tokio::select! {
        biased;
        () = reply.closed() => None,
        result = query => Some(result),
    };
    process.control.request_close();
    // Keep this helper's admission until its original OS scope is proven
    // gone, even when the observer or a cleanup observation times out.
    while process.control.closed().await.is_err() {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    result.transpose()
}

async fn fetch_kimi_usage(port: u16, token: &str) -> Result<serde_json::Value, DriverError> {
    const BODY_LIMIT: usize = 1_048_576;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| DriverError::Protocol("Kimi HTTP client unavailable".into()))?;
    let mut response = client
        .get(format!("http://127.0.0.1:{port}/api/v1/oauth/usage"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| DriverError::Disconnected("Kimi usage request failed or timed out".into()))?;
    if !response.status().is_success() {
        return Err(DriverError::Protocol(
            "Kimi usage endpoint returned an unsuccessful status".into(),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| DriverError::Protocol("Kimi usage response interrupted".into()))?
    {
        if body.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err(DriverError::Protocol(
                "Kimi usage response exceeded size limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| DriverError::Protocol("Kimi usage endpoint returned invalid JSON".into()))
}

fn driver_error_from_channel() -> DriverError {
    DriverError::Disconnected("ACP agent connection is closed".into())
}

/// The last agent stderr lines that mention errors. They stay in memory only
/// and are never displayed or persisted; they exist because some agents (for
/// example Grok with an exhausted token quota) kill their worker before the
/// JSON-RPC layer can relay a usable failure reason, so stderr carries the
/// only signal that separates "quota used up" from "unusable response".
#[derive(Clone, Default)]
struct StderrTail(Arc<std::sync::Mutex<StderrLines>>);

#[derive(Default)]
struct StderrLines {
    next_sequence: u64,
    lines: std::collections::VecDeque<(u64, String)>,
}

struct RequestStderr<'a> {
    tail: &'a StderrTail,
    start_sequence: u64,
}

const STDERR_TAIL_CAPACITY: usize = 8;
const STDERR_LINE_MAX_CHARS: usize = 300;

impl StderrTail {
    fn record(&self, line: &str) {
        let line = line.trim();
        if line.is_empty() || !line.to_ascii_lowercase().contains("error") {
            return;
        }
        let mut tail = self.0.lock().expect("stderr tail");
        if tail.lines.len() == STDERR_TAIL_CAPACITY {
            tail.lines.pop_front();
        }
        let sequence = tail.next_sequence;
        tail.next_sequence += 1;
        tail.lines
            .push_back((sequence, line.chars().take(STDERR_LINE_MAX_CHARS).collect()));
    }
    fn clear(&self) {
        self.0.lock().expect("stderr tail").lines.clear();
    }
    fn begin_request(&self) -> RequestStderr<'_> {
        RequestStderr {
            tail: self,
            start_sequence: self.0.lock().expect("stderr tail").next_sequence,
        }
    }
}

impl RequestStderr<'_> {
    fn failure(&self) -> Option<DriverError> {
        // Do not clear the shared tail: another in-flight request may still need it.
        // Only lines recorded since this request began can explain its failure.
        let tail = self.tail.0.lock().expect("stderr tail");
        let mut lines = tail
            .lines
            .iter()
            .rev()
            .filter(|(sequence, _)| *sequence >= self.start_sequence);
        if lines
            .clone()
            .any(|(_, line)| explicitly_reports_quota_exhaustion(line))
        {
            Some(DriverError::QuotaExhausted {
                resets_at_unix: None,
            })
        } else {
            lines
                .find_map(|(_, line)| driver_api::failure::classify(line))
                .map(|kind| DriverError::Protocol(kind.into()))
        }
    }
}

/// Session-level ACP errors split into `NotConfigured` (login missing) and
/// generic Protocol so the desktop can show the honest installation state.
fn map_session_error(
    error: &AcpError,
    context: &'static str,
    stderr: Option<&RequestStderr<'_>>,
) -> DriverError {
    let message = error.message.to_ascii_lowercase();
    if message.contains("auth") || message.contains("login") || message.contains("unauthorized") {
        DriverError::NotConfigured("agent requires login before sessions can be created".into())
    } else if explicitly_reports_quota_exhaustion(&error.message) {
        DriverError::QuotaExhausted {
            resets_at_unix: None,
        }
    } else if let Some(kind) = driver_api::failure::classify(&error.message) {
        DriverError::Protocol(kind.into())
    } else {
        let from_stderr = stderr.and_then(RequestStderr::failure);
        from_stderr.unwrap_or_else(|| DriverError::Protocol(context.into()))
    }
}

fn prompt_stop_reason(done: PromptResponse) -> Result<StopReason, DriverError> {
    // CodeBuddy can report its native cancellation/failure in the vendor
    // outcome even when the ACP stop reason is end_turn. Do not reuse that
    // connection as though a normal turn completed.
    match done
        .meta
        .as_ref()
        .and_then(|meta| meta.get("codebuddy.ai/outcome"))
        .and_then(serde_json::Value::as_str)
    {
        Some("CANCELLED") => Ok(StopReason::Cancelled),
        Some("SUCCESS" | "PARTIAL_SUCCESS") | None => Ok(done.stop_reason),
        Some(_) => Err(DriverError::Protocol(
            "CodeBuddy reported an unsuccessful native turn".into(),
        )),
    }
}

fn prompt_terminal(
    result: Result<StopReason, DriverError>,
    is_grok: bool,
    approval_rejected: bool,
    interrupt_requested: bool,
) -> DriverEvent {
    match result {
        Ok(StopReason::Cancelled | StopReason::Refusal)
            if is_grok && approval_rejected && !interrupt_requested =>
        {
            DriverEvent::TurnCompleted {
                summary: "已拒绝工具操作，本轮已结束".into(),
            }
        }
        Ok(StopReason::EndTurn) => DriverEvent::TurnCompleted {
            summary: "ACP turn completed".into(),
        },
        Ok(StopReason::Cancelled | StopReason::Refusal) => DriverEvent::TurnInterrupted {
            summary: "ACP turn ended without completing".into(),
        },
        Ok(_) => DriverEvent::TurnFailed {
            reason: "ACP turn ended with a failure stop reason".into(),
        },
        Err(DriverError::QuotaExhausted { resets_at_unix }) => {
            DriverEvent::TurnQuotaExhausted { resets_at_unix }
        }
        Err(DriverError::CapabilityUnavailable(reason)) => DriverEvent::TurnFailed { reason },
        Err(error) => DriverEvent::TurnFailed {
            reason: driver_api::failure::turn_failed_reason(&error),
        },
    }
}

type Updates = Arc<std::sync::Mutex<Option<mpsc::UnboundedSender<DriverEvent>>>>;

impl ConnectionHandle {
    fn unavailable(&self) -> bool {
        self.finished.load(Ordering::Acquire)
            || self.native.as_ref().is_some_and(|native| native.stopping())
            || self.commands.is_closed()
            || self.cancels.is_closed()
    }

    // Retain cleanup ownership without keeping the old connection's senders alive.
    fn cleanup_marker(&self) -> Self {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (cancels, cancel_rx) = mpsc::unbounded_channel();
        drop(command_rx);
        drop(cancel_rx);
        Self {
            codebuddy_cwd: self.codebuddy_cwd.clone(),
            cursor_force: self.cursor_force,
            commands,
            cancels,
            finished: Arc::clone(&self.finished),
            native: self.native.clone(),
        }
    }
}

enum Command {
    ProbeReady {
        reply: oneshot::Sender<Result<ProbeReport, DriverError>>,
    },
    PermissionCatalog {
        workspace: Option<PathBuf>,
        reply: oneshot::Sender<Result<driver_api::PermissionCatalog, DriverError>>,
    },
    Catalog {
        model: Option<String>,
        workspace: Option<PathBuf>,
        reply: oneshot::Sender<Result<driver_api::ModelCatalog, DriverError>>,
    },
    NewSession {
        history_mcp: Option<driver_api::HistoryMcp>,
        grok: bool,
        permission_mode: Option<String>,
        model: Option<String>,
        thinking: Option<String>,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
        reply: oneshot::Sender<Result<SessionId, DriverError>>,
    },
    LoadSession {
        history_mcp: Option<driver_api::HistoryMcp>,
        grok: bool,
        permission_mode: Option<String>,
        session_id: SessionId,
        model: Option<String>,
        thinking: Option<String>,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
        reply: oneshot::Sender<Result<SessionId, DriverError>>,
    },
    CloseSession {
        session_id: String,
        reply: oneshot::Sender<Result<(), DriverError>>,
    },
    ForgetSession {
        session_id: String,
        reply: oneshot::Sender<()>,
    },
    Prompt {
        session_id: SessionId,
        blocks: Vec<ContentBlock>,
        image_compatibility: bool,
        reply: oneshot::Sender<Result<StopReason, DriverError>>,
        turn: Option<Arc<turn::Turn>>,
    },
    ReadGrokBilling {
        reply: oneshot::Sender<Result<serde_json::Value, DriverError>>,
    },
    ReadKiroContext {
        scope: auxiliary::Call,
        reply: oneshot::Sender<Result<serde_json::Value, DriverError>>,
    },
    ReadKiroUsage {
        reply: oneshot::Sender<Result<serde_json::Value, DriverError>>,
    },
}

#[derive(Clone)]
struct ConnectionHandle {
    codebuddy_cwd: Option<PathBuf>,
    cursor_force: bool,
    commands: mpsc::UnboundedSender<Command>,
    cancels: mpsc::UnboundedSender<Arc<turn::Turn>>,
    finished: Arc<AtomicBool>,
    native: Option<Arc<lifecycle::Control>>,
}

/// One outstanding vendor `session/request_permission` request: the SDK
/// responder stays parked here until the operator picks an option.
struct PendingPermission {
    responder: Responder<RequestPermissionResponse>,
    options: Vec<PermissionOption>,
    session_id: String,
    tool_call_id: String,
}

type PendingPermissions = Arc<std::sync::Mutex<HashMap<String, PendingPermission>>>;

struct PendingQuestion {
    session_id: String,
    form: agent_domain::QuestionForm,
    reply: oneshot::Sender<agent_domain::QuestionAnswer>,
}
type PendingQuestions = Arc<std::sync::Mutex<HashMap<String, PendingQuestion>>>;

struct Shared {
    launch: AcpLaunch,
    connection: tokio::sync::Mutex<Option<ConnectionHandle>>,
    lifecycle: std::sync::Mutex<()>,
    setup: Arc<tokio::sync::Mutex<()>>,
    metadata: Arc<tokio::sync::Mutex<()>>,
    setup_in_progress: AtomicBool,
    native_close: AtomicBool,
    pending_auxiliary: Arc<std::sync::atomic::AtomicUsize>,
    sessions: std::sync::Mutex<HashMap<String, Arc<sessions::Resident>>>,
    active_session: std::sync::Mutex<Option<String>>,
    latest_turn: std::sync::Mutex<Option<Arc<turn::Turn>>>,
    closed_sessions: std::sync::Mutex<HashSet<String>>,
    retired_handles: std::sync::Mutex<HashMap<String, SessionHandle>>,
    turn_active: AtomicBool,
    approval_rejected: AtomicBool,
    interrupt_requested: AtomicBool,
    reconnect_required: AtomicBool,
    updates: Updates,
    pending_permissions: PendingPermissions,
    questions: PendingQuestions,
    activity: Arc<std::sync::Mutex<activity::Activities>>,
    quota: Arc<AcpQuotaState>,
}

impl Shared {
    // Unlike a permission decision, terminal progress is still valid while
    // interrupting. Explicitly retired sessions may never route to a new sink.
    fn accepts_update(&self, session: Option<&str>) -> bool {
        if session.is_none() && self.pending_auxiliary.load(Ordering::Acquire) > 0 {
            return false;
        }
        self.active_session
            .lock()
            .expect("active session")
            .as_deref()
            .is_some_and(|active| {
                session.is_none_or(|session| session == active)
                    && !self
                        .closed_sessions
                        .lock()
                        .expect("closed sessions")
                        .contains(active)
            })
    }
    // Called under lifecycle so admission, stopping, settlement and a response
    // cannot race. Vendors without a question session id bind to the sole turn.
    fn validate_interaction(&self, session: Option<&str>) -> Result<(), DriverError> {
        if session.is_none() && self.pending_auxiliary.load(Ordering::Acquire) > 0 {
            return Err(DriverError::CapabilityUnavailable(
                "ACP interaction has no session identity during a pending query".into(),
            ));
        }
        let active = self.active_session.lock().expect("active session");
        if active.as_deref().is_none_or(|active| {
            session.is_some_and(|session| session != active)
                || self
                    .closed_sessions
                    .lock()
                    .expect("closed sessions")
                    .contains(active)
        }) {
            return Err(DriverError::SessionMissing(
                "ACP interaction session is not active".into(),
            ));
        }
        if self.interrupt_requested.load(Ordering::Acquire) {
            return Err(DriverError::CapabilityUnavailable(
                "ACP session is stopping".into(),
            ));
        }
        Ok(())
    }

    fn cancel_interactions(&self, session: &str) {
        let permissions = {
            let mut pending = self
                .pending_permissions
                .lock()
                .expect("pending permissions");
            let ids: Vec<_> = pending
                .iter()
                .filter(|(_, value)| value.session_id == session)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| pending.remove(&id))
                .collect::<Vec<_>>()
        };
        for pending in permissions {
            let _ = pending.responder.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }
        // Dropping a question responder resolves the tracked vendor bridge as
        // cancelled. Never manufacture an allow or a user answer when stopping.
        self.questions
            .lock()
            .expect("questions")
            .retain(|_, question| {
                !question.session_id.is_empty() && question.session_id != session
            });
    }
}

#[derive(Default)]
struct AcpQuotaState {
    grok_usage: std::sync::Mutex<grok_usage::GrokUsage>,
    directories: std::sync::Mutex<HashMap<String, PathBuf>>,
    usage: driver_api::usage::UsageStore,
    kiro_credits: std::sync::Mutex<HashMap<String, HashMap<String, f64>>>,
    context_by_session: std::sync::Mutex<HashMap<String, (i64, ContextUsageSnapshot)>>,
    account_snapshot: std::sync::Mutex<Option<QuotaSnapshot>>,
    refresh: tokio::sync::Mutex<()>,
    last_error: std::sync::Mutex<Option<DriverError>>,
    account_fresh_until_unix_ms: std::sync::atomic::AtomicI64,
    stderr_tail: StderrTail,
}

impl AcpQuotaState {
    fn observe_usage(&self, notification: &SessionNotification) {
        let raw = serde_json::to_value(notification).unwrap_or_default();
        let id = notification.session_id.to_string();
        self.observe_model(&id, &raw["update"]);
        self.usage
            .merge(&id, driver_api::usage::acp(&raw["update"]));
        self.observe_kiro(&id, &raw["update"]["_meta"]["kiro"]);
        let SessionUpdate::UsageUpdate(usage) = &notification.update else {
            return;
        };
        if usage.size == 0 {
            return;
        }
        let collected_at_unix_ms = now_unix_ms();
        self.context_by_session.lock().unwrap().insert(
            notification.session_id.0.to_string(),
            (
                collected_at_unix_ms,
                ContextUsageSnapshot {
                    used_tokens: usage.used,
                    window_tokens: usage.size,
                    cache_hit_rate_percent: None,
                },
            ),
        );
    }

    /// Context readings older than the TTL are treated as absent: the UI must
    /// not show stale context windows as if they were live.
    fn context(&self, native_session_id: Option<&str>) -> Option<ContextUsageSnapshot> {
        let session_id = native_session_id?;
        let now = now_unix_ms();
        let guard = self.context_by_session.lock().unwrap();
        let &(collected_at_unix_ms, ref snapshot) = guard.get(session_id)?;
        (now - collected_at_unix_ms <= ACP_CONTEXT_TTL_MS).then_some(snapshot.clone())
    }

    fn account_snapshot(&self, native_session_id: Option<&str>) -> Option<QuotaSnapshot> {
        let mut snapshot = self.account_snapshot.lock().unwrap().clone()?;
        snapshot.context = self.context(native_session_id);
        Some(snapshot)
    }

    fn account_is_fresh(&self) -> bool {
        now_unix_ms() < self.account_fresh_until_unix_ms.load(Ordering::Acquire)
    }

    fn record_account(&self, snapshot: QuotaSnapshot) {
        *self.account_snapshot.lock().unwrap() = Some(snapshot);
        self.account_fresh_until_unix_ms.store(
            now_unix_ms().saturating_add(GROK_QUOTA_TTL_MS_I64),
            Ordering::Release,
        );
    }
}

/// Where the ACP server binary comes from: a direct executable plus structured
/// arguments (for example `kimi acp` or `grok agent stdio`).
#[derive(Clone, Debug)]
pub struct AcpLaunch {
    pub executable: PathBuf,
    pub args: Vec<String>,
}

pub struct AcpDriver {
    executor: ExecutorRef,
    native_version: Option<String>,
    resume_capability: SessionResumeCapability,
    shared: Arc<Shared>,
    stopping: tokio::sync::watch::Sender<bool>,
    #[cfg(test)]
    stop_timeout: std::time::Duration,
    #[cfg(test)]
    metadata_timeout: std::time::Duration,
}

impl Drop for AcpDriver {
    fn drop(&mut self) {
        self.stopping.send_replace(true);
    }
}

impl AcpDriver {
    async fn configured_session(
        &self,
        connection: ConnectionHandle,
        command: Command,
        response: oneshot::Receiver<Result<SessionId, DriverError>>,
        permit: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<SessionHandle, DriverError> {
        let target = match &command {
            Command::LoadSession { session_id, .. } => Some(session_id.to_string()),
            _ => None,
        };
        {
            let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
            self.no_pending_stop()?;
            self.shared.setup_available()?;
            self.shared.setup_in_progress.store(true, Ordering::Release);
            if connection.commands.send(command).is_err() {
                self.shared
                    .setup_in_progress
                    .store(false, Ordering::Release);
                return Err(driver_error_from_channel());
            }
        }
        sessions::finish_setup(self.shared.clone(), connection, response, permit, target).await
    }
    fn no_pending_stop(&self) -> Result<(), DriverError> {
        if self
            .shared
            .latest_turn
            .lock()
            .expect("ACP turn")
            .as_ref()
            .is_some_and(|turn| turn.stop_pending())
        {
            return Err(DriverError::CapabilityUnavailable(
                "ACP stop is still awaiting confirmation".into(),
            ));
        }
        Ok(())
    }
    /// Creates a driver for one concrete installation. `resume_capability` must
    /// come from a real `initialize` handshake (`probe_ready`), never from docs.
    pub fn new(
        provider_id: &str,
        installation_id: &str,
        launch: AcpLaunch,
        native_version: Option<String>,
        resume_capability: SessionResumeCapability,
    ) -> Result<Self, agent_domain::DomainError> {
        Ok(Self {
            executor: ExecutorRef {
                provider_id: ProviderId::parse(provider_id)?,
                installation_id: Some(
                    agent_domain::InstallationId::parse(installation_id)
                        .map_err(|error| {
                            agent_domain::DomainError::InvalidProviderId(error.to_string())
                        })?
                        .to_string(),
                ),
            },
            native_version,
            resume_capability,
            stopping: tokio::sync::watch::channel(false).0,
            #[cfg(test)]
            stop_timeout: std::time::Duration::from_secs(10),
            #[cfg(test)]
            metadata_timeout: std::time::Duration::from_secs(30),
            shared: Arc::new(Shared {
                activity: Arc::new(std::sync::Mutex::new(activity::Activities::default())),
                launch,
                connection: tokio::sync::Mutex::new(None),
                lifecycle: std::sync::Mutex::new(()),
                setup: Arc::new(tokio::sync::Mutex::new(())),
                metadata: Arc::new(tokio::sync::Mutex::new(())),
                setup_in_progress: AtomicBool::new(false),
                native_close: AtomicBool::new(false),
                pending_auxiliary: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                sessions: std::sync::Mutex::new(HashMap::new()),
                active_session: std::sync::Mutex::new(None),
                latest_turn: std::sync::Mutex::new(None),
                closed_sessions: std::sync::Mutex::new(HashSet::new()),
                retired_handles: std::sync::Mutex::new(HashMap::new()),
                turn_active: AtomicBool::new(false),
                approval_rejected: AtomicBool::new(false),
                interrupt_requested: AtomicBool::new(false),
                reconnect_required: AtomicBool::new(false),
                updates: Arc::new(std::sync::Mutex::new(None)),
                pending_permissions: Arc::new(std::sync::Mutex::new(HashMap::new())),
                questions: Arc::new(std::sync::Mutex::new(HashMap::new())),
                quota: Arc::new(AcpQuotaState::default()),
            }),
        })
    }

    /// Authenticates with a metadata session and keeps the initialized process.
    /// Actual tasks still create distinct sessions with their own workspace/MCP.
    pub async fn connect_ready(
        provider_id: &str,
        installation_id: &str,
        launch: AcpLaunch,
        fallback_version: Option<String>,
    ) -> Result<Self, DriverError> {
        let mut driver = Self::new(
            provider_id,
            installation_id,
            launch,
            fallback_version,
            SessionResumeCapability::Unsupported,
        )
        .map_err(|_| DriverError::Protocol("invalid ACP installation identity".into()))?;
        let report = tokio::time::timeout(std::time::Duration::from_secs(45), async {
            let connection = driver.ensure_connection().await?;
            let (reply, response) = oneshot::channel();
            connection
                .commands
                .send(Command::ProbeReady { reply })
                .map_err(|_| driver_error_from_channel())?;
            response.await.map_err(|_| driver_error_from_channel())?
        })
        .await
        .map_err(|_| DriverError::Disconnected("ACP readiness probe timed out".into()))??;
        driver.native_version = if provider_id == "deepseek-harness" {
            // DSH reports the ACP module version (0.0.1), not the CLI release.
            driver.native_version.take().or(report.native_version)
        } else {
            report.native_version.or(driver.native_version.take())
        };
        driver.resume_capability = report.resume_capability;
        Ok(driver)
    }

    /// Runs one ephemeral initialize handshake against the launch command to
    /// prove the binary actually speaks ACP stable v1. No model turn is started.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::NotConfigured`] when the process cannot start and
    /// [`DriverError::Protocol`] when the handshake fails or negotiates a
    /// protocol version this driver does not support.
    pub async fn probe_ready(launch: &AcpLaunch) -> Result<ProbeReport, DriverError> {
        let (mut child, transport) = spawn_agent(launch)?;
        let started = std::time::Instant::now();
        // ACP initialize does not require authentication, but session/new
        // does; the probe therefore proves the agent can actually open a
        // session, so unauthenticated installs report NotConfigured instead
        // of a Ready that fails on the first turn.
        let probe = tokio::time::timeout(
            // A cold Cursor session also fetches model/configuration data;
            // real installations can take over 20 seconds after initialize.
            std::time::Duration::from_secs(45),
            Client
                .builder()
                .connect_with(transport, async |connection| {
                    let initialized = initialize(&connection).await?;
                    eprintln!(
                        "acp timing stage=initialize elapsed_ms={}",
                        started.elapsed().as_millis()
                    );
                    let session = connection
                        .send_request(NewSessionRequest::new(std::env::temp_dir()))
                        .block_task()
                        .await?;
                    eprintln!(
                        "acp timing stage=probe_session elapsed_ms={}",
                        started.elapsed().as_millis()
                    );
                    Ok((initialized, session.session_id))
                }),
        )
        .await;
        // Native EOF gets a grace period, but only the original scope's exit
        // proof lets this temporary probe report readiness.
        child.kill().await?;
        let probe =
            probe.map_err(|_| DriverError::Disconnected("ACP readiness probe timed out".into()))?;
        match probe {
            Ok((response, _)) if response.protocol_version == ProtocolVersion::V1 => {
                Ok(ProbeReport {
                    resume_capability: if supports_native_resume(
                        &serde_json::to_value(&response).unwrap_or_default(),
                    ) {
                        SessionResumeCapability::Supported
                    } else {
                        SessionResumeCapability::Unsupported
                    },
                    native_version: response
                        .agent_info
                        .as_ref()
                        .map(|info| info.version.clone()),
                })
            }
            Ok(_) => Err(DriverError::Protocol(
                "agent negotiated an unsupported ACP protocol version".into(),
            )),
            Err(error) => Err(map_session_error(
                &error,
                "agent rejected the ACP initialize or session handshake",
                None,
            )),
        }
    }

    async fn ensure_connection(&self) -> Result<ConnectionHandle, DriverError> {
        self.ensure_permission_connection(None).await
    }

    async fn ensure_permission_connection(
        &self,
        mode: Option<&str>,
    ) -> Result<ConnectionHandle, DriverError> {
        self.ensure_workspace_connection(mode, None).await
    }
    async fn ensure_workspace_connection(
        &self,
        mode: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<ConnectionHandle, DriverError> {
        if self.executor.provider_id.as_str() == "cursor" {
            permissions::cursor_catalog().validate(mode)?;
            if mode.is_some() && self.shared.turn_active.load(Ordering::Acquire) {
                return Err(DriverError::CapabilityUnavailable(
                    "请在 Cursor 空闲时切换权限策略".into(),
                ));
            }
        }
        let mut guard = self.shared.connection.lock().await;
        let codebuddy_cwd = if self.executor.provider_id.as_str() == "codebuddy" {
            cwd.map(PathBuf::from)
                .or_else(|| guard.as_ref().and_then(|c| c.codebuddy_cwd.clone()))
        } else {
            None
        };
        if self.shared.turn_active.load(Ordering::Acquire)
            && guard.as_ref().is_some_and(|c| {
                c.unavailable()
                    || c.codebuddy_cwd != codebuddy_cwd
                    || self.shared.reconnect_required.load(Ordering::Acquire)
            })
        {
            return Err(DriverError::CapabilityUnavailable(
                "当前会话运行中，无法重建 Agent 连接".into(),
            ));
        }
        let force = if self.executor.provider_id.as_str() == "cursor" {
            mode.map_or_else(
                || guard.as_ref().is_some_and(|c| c.cursor_force),
                |m| m == "full-access",
            )
        } else {
            false
        };
        if self.shared.setup_in_progress.load(Ordering::Acquire)
            && guard.as_ref().is_some_and(|existing| {
                existing.unavailable()
                    || existing.cursor_force != force
                    || existing.codebuddy_cwd != codebuddy_cwd
                    || self.shared.reconnect_required.load(Ordering::Acquire)
            })
        {
            return Err(DriverError::CapabilityUnavailable(
                "ACP configuration is still being confirmed; cannot rebuild its connection".into(),
            ));
        }
        if let Some(existing) = guard.as_ref()
            && !existing.unavailable()
            && (existing.cursor_force != force
                || existing.codebuddy_cwd != codebuddy_cwd
                || self.shared.reconnect_required.load(Ordering::Acquire))
            && self
                .shared
                .sessions
                .lock()
                .expect("ACP sessions")
                .values()
                .filter(|resident| {
                    Arc::ptr_eq(&resident.epoch, &existing.finished) && !resident.is_closing()
                })
                .count()
                > 1
        {
            return Err(DriverError::CapabilityUnavailable(
                "Other ACP sessions still own this connection; close them before rebuilding it"
                    .into(),
            ));
        }
        if guard.as_ref().is_some_and(|c| {
            c.unavailable()
                || c.cursor_force != force
                || c.codebuddy_cwd != codebuddy_cwd
                || self.shared.reconnect_required.load(Ordering::Acquire)
        }) {
            let old = guard.take().unwrap();
            let finished = Arc::clone(&old.finished);
            // A timeout/cancel must not let the next restore spawn over an unconfirmed exit.
            *guard = Some(old.cleanup_marker());
            if let Some(native) = &old.native {
                native.request_close();
            }
            drop(old);
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                while !finished.load(Ordering::Acquire) {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .map_err(|_| DriverError::Disconnected("Agent 连接重建时旧进程未退出".into()))?;
            guard.take();
        }
        if let Some(existing) = guard.as_ref()
            && !existing.finished.load(Ordering::Acquire)
        {
            return Ok(ConnectionHandle {
                codebuddy_cwd: codebuddy_cwd.clone(),
                cursor_force: force,
                commands: existing.commands.clone(),
                cancels: existing.cancels.clone(),
                finished: Arc::clone(&existing.finished),
                native: existing.native.clone(),
            });
        }
        self.shared.native_close.store(false, Ordering::Release);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
        let finished = Arc::new(AtomicBool::new(false));
        let mut launch = self.shared.launch.clone();
        if self.executor.provider_id.as_str() == "cursor" {
            permissions::cursor_launch_args(&mut launch.args, force);
        }
        self.shared.quota.stderr_tail.clear();
        let (mut child, transport) = spawn_agent_in(
            &launch,
            codebuddy_cwd.as_deref(),
            Some(self.shared.quota.stderr_tail.clone()),
        )?;
        child.control.observe_owner(self.stopping.subscribe());
        let native = child.control.clone();
        let background = lifecycle::Tasks::new();
        if self.executor.provider_id.as_str() == "grok" {
            self.shared
                .quota
                .grok_usage
                .lock()
                .expect("grok usage")
                .reset_connection();
        }
        let shared = ConnectionTasks {
            owner: Some(Arc::downgrade(&self.shared)),
            updates: Arc::clone(&self.shared.updates),
            pending: Arc::clone(&self.shared.pending_permissions),
            questions: Arc::clone(&self.shared.questions),
            activity: self.shared.activity.clone(),
            quota: Arc::clone(&self.shared.quota),
            background: background.clone(),
        };
        let finished_flag = Arc::clone(&finished);
        let pending = self.shared.pending_permissions.clone();
        let questions = self.shared.questions.clone();
        tokio::spawn(async move {
            let mut client = tokio::spawn(run_connection(transport, command_rx, cancel_rx, shared));
            tokio::select! {_= &mut client=>{},()=child.control.closing()=>{client.abort();let _=client.await;}}
            background.shutdown().await;
            pending.lock().expect("pending permissions").clear();
            questions.lock().expect("questions").clear();
            while child.kill().await.is_err() {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            finished_flag.store(true, Ordering::Release);
        });
        let handle = ConnectionHandle {
            codebuddy_cwd: codebuddy_cwd.clone(),
            cursor_force: force,
            commands: command_tx,
            cancels: cancel_tx,
            finished: Arc::clone(&finished),
            native: Some(native.clone()),
        };
        *guard = Some(ConnectionHandle {
            codebuddy_cwd,
            cursor_force: force,
            commands: handle.commands.clone(),
            cancels: handle.cancels.clone(),
            finished: Arc::clone(&finished),
            native: Some(native),
        });
        self.shared
            .reconnect_required
            .store(false, Ordering::Release);
        Ok(handle)
    }
}

struct ConnectionTasks {
    owner: Option<std::sync::Weak<Shared>>,
    updates: Updates,
    pending: PendingPermissions,
    questions: PendingQuestions,
    activity: Arc<std::sync::Mutex<activity::Activities>>,
    quota: Arc<AcpQuotaState>,
    background: Arc<lifecycle::Tasks>,
}

pub struct ProbeReport {
    pub resume_capability: SessionResumeCapability,
    pub native_version: Option<String>,
}

fn supports_native_resume(initial: &serde_json::Value) -> bool {
    initial["agentCapabilities"]["loadSession"] == true
        || initial
            .pointer("/agentCapabilities/sessionCapabilities/resume")
            .is_some_and(serde_json::Value::is_object)
}
fn resume_method(initial: &serde_json::Value) -> &'static str {
    if initial["agentCapabilities"]["loadSession"] == true {
        "session/load"
    } else {
        "session/resume"
    }
}

async fn initialize(connection: &ConnectionTo<Agent>) -> Result<InitializeResponse, AcpError> {
    let initialized = connection
        .send_request(serde_json::from_value::<InitializeRequest>(serde_json::json!({"protocolVersion":1,"clientCapabilities":{"elicitation":{"form":{}},"_meta":{"codebuddy.ai":{"question":true}}}})).expect("valid initialization capabilities"))
        .block_task()
        .await?;
    let value = serde_json::to_value(&initialized).unwrap_or_default();
    if models::has_kiro_template(&value) {
        // Kiro CLI auth reuses its logged-in account and starts async model
        // discovery. No browser-based login is initiated by this adapter.
        if let Some(method) = value["authMethods"].as_array().and_then(|v| v.first()) {
            connection
                .send_request(agent_client_protocol::UntypedMessage::new(
                    "authenticate",
                    serde_json::json!({"methodId":method["id"]}),
                )?)
                .block_task()
                .await?;
        }
    }
    Ok(initialized)
}

type AgentTransport = ByteStreams<Compat<lifecycle::Input>, Compat<lifecycle::Output>>;

fn spawn_agent(launch: &AcpLaunch) -> Result<(lifecycle::Process, AgentTransport), DriverError> {
    spawn_agent_in(launch, None, None)
}
fn spawn_agent_in(
    launch: &AcpLaunch,
    cwd: Option<&std::path::Path>,
    stderr_tail: Option<StderrTail>,
) -> Result<(lifecycle::Process, AgentTransport), DriverError> {
    let mut command = tokio::process::Command::new(&launch.executable);
    command
        .args(&launch.args)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    agent_runtime::apply_platform_environment(&mut command);
    for variable in ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(variable) {
            command.env(variable, value);
        }
    }
    agent_runtime::apply_proxy_environment(&mut command);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(path) = agent_runtime::installed_node_path(&launch.executable) {
        command.env("PATH", path);
    }
    let mut child = agent_runtime::OwnedChild::spawn(command).map_err(|error| {
        if matches!(error.kind(), std::io::ErrorKind::NotFound) {
            DriverError::NotConfigured("ACP agent executable is missing".into())
        } else {
            DriverError::Disconnected(format!("ACP agent process failed to start: {error}"))
        }
    })?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| DriverError::Disconnected("agent stdin is unavailable".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| DriverError::Disconnected("agent stdout is unavailable".into()))?;
    let stderr = child.stderr.take().map(|stderr| {
        // Drain stderr so a chatty agent can never block on a full pipe; the
        // content is diagnostics-only and is never persisted. Error lines are
        // kept briefly in memory to sharpen failure classification.
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut stderr = stderr;
            let mut pending = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => pending.extend_from_slice(&chunk[..read]),
                }
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    if let Some(tail) = &stderr_tail {
                        tail.record(&String::from_utf8_lossy(&line[..line.len() - 1]));
                    }
                }
                // A pathologically long line without newlines must not grow forever.
                if pending.len() > 64 * 1024 {
                    pending.clear();
                }
            }
            if let Some(tail) = &stderr_tail {
                tail.record(&String::from_utf8_lossy(&pending));
            }
        })
    });
    let process = lifecycle::Process::start(child, stderr);
    let input = lifecycle::Input {
        inner: stdin,
        control: process.control.clone(),
    };
    let output = lifecycle::Output {
        inner: stdout,
        control: process.control.clone(),
        line_bytes: 0,
    };
    Ok((
        process,
        ByteStreams::new(input.compat_write(), output.compat()),
    ))
}

async fn run_connection(
    transport: ByteStreams<
        impl futures::AsyncWrite + Send + 'static,
        impl futures::AsyncRead + Send + 'static,
    >,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut cancels: mpsc::UnboundedReceiver<Arc<turn::Turn>>,
    tasks: ConnectionTasks,
) {
    let permission_tasks = ConnectionTasks {
        owner: tasks.owner.clone(),
        updates: Arc::clone(&tasks.updates),
        pending: Arc::clone(&tasks.pending),
        questions: Arc::clone(&tasks.questions),
        activity: tasks.activity.clone(),
        quota: Arc::clone(&tasks.quota),
        background: tasks.background.clone(),
    };
    let command_quota = Arc::clone(&tasks.quota);
    let vendor_quota = Arc::clone(&tasks.quota);
    let vendor_updates = Arc::clone(&tasks.updates);
    let vendor_owner = tasks.owner.clone();
    let cursor_updates = Arc::clone(&tasks.updates);
    let cursor_activity = tasks.activity.clone();
    let cursor_owner = tasks.owner.clone();
    let session_states = Arc::new(std::sync::Mutex::new(
        session_state::SessionStates::default(),
    ));
    let update_states = Arc::clone(&session_states);
    let questions = Arc::clone(&tasks.questions);
    let question_owner = tasks.owner.clone();
    let question_updates = Arc::clone(&tasks.updates);
    let vendor_background = tasks.background.clone();
    let question_background = tasks.background.clone();
    let command_background = tasks.background.clone();
    let command_owner = tasks.owner.clone();
    let result = Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                let owner = tasks.owner.as_ref().and_then(std::sync::Weak::upgrade);
                let _gate = owner.as_ref().map(|owner| owner.lifecycle.lock().expect("ACP lifecycle"));
                update_states.lock().expect("session states").observe(&serde_json::to_value(&notification).unwrap_or_default());
                tasks.quota.observe_usage(&notification);
                let raw = serde_json::to_value(&notification).unwrap_or_default();
                let records = tasks.quota.observe_live_grok(&raw);
                if tasks.owner.is_some() && owner.as_ref().is_none_or(|owner| !owner.accepts_update(Some(notification.session_id.0.as_ref()))) { return Ok(()); }
                if let Some(updates) = tasks.updates.lock().expect("updates sink").as_ref() {
                    for request in records { let _ = updates.send(DriverEvent::UsageRecorded { request }); }
                    let raw=serde_json::to_value(&notification).unwrap_or_default();
                    for event in tasks.activity.lock().expect("activity").update(&raw) { let _ = updates.send(event); }
                    for event in map_session_update(&notification) {
                        if !matches!(event, DriverEvent::ToolStarted{..}|DriverEvent::ToolCompleted{..}|DriverEvent::TextDelta{..}) { let _ = updates.send(event); }
                    }
                }
                Ok(())
            },
            on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: agent_client_protocol::schema::v1::RequestPermissionRequest,
                        responder: Responder<RequestPermissionResponse>,
                        connection| {
                let owner = permission_tasks.owner.as_ref().and_then(std::sync::Weak::upgrade);
                let _gate = owner.as_ref().map(|owner| owner.lifecycle.lock().expect("ACP lifecycle"));
                if permission_tasks.owner.is_some() && owner.as_ref().is_none_or(|owner| {
                    owner.validate_interaction(Some(request.session_id.0.as_ref())).is_err()
                }) {
                    responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))?;
                    return Ok(agent_client_protocol::Handled::Yes);
                }
                let raw=serde_json::to_value(&request).unwrap_or_default();
                if raw["toolCall"]["_meta"]["codebuddy.ai/toolName"]=="AskUserQuestion" {
                    let params=raw["toolCall"]["rawInput"].clone();
                    if let Ok(form)=driver_api::questions::native_form("codebuddy",&params){
                        let id=format!("acp-question-{}",uuid_question_id());
                        let (reply,receiver)=oneshot::channel();
                        permission_tasks.questions.lock().expect("questions").insert(id.clone(),PendingQuestion{session_id:request.session_id.to_string(),form:form.clone(),reply});
                        let delivered=permission_tasks.updates.lock().expect("updates").as_ref().is_some_and(|u|u.send(DriverEvent::QuestionRequested{native_request_id:id.clone(),form}).is_ok());
                        if !delivered{permission_tasks.questions.lock().expect("questions").remove(&id);}
                        let pending=Arc::clone(&permission_tasks.questions);
                        let bridge=owner.as_ref().and_then(codebuddy_question::Bridge::capture);
                        permission_tasks.background.spawn(async move {
                            let answer=receiver.await.unwrap_or(agent_domain::QuestionAnswer{cancelled:true,values:serde_json::Value::Null});
                            pending.lock().expect("questions").remove(&id);
                            let native=driver_api::questions::native_answer("codebuddy",&params,&answer);
                            let mut payload=serde_json::json!({"sessionId":request.session_id,"toolCallId":request.tool_call.tool_call_id,"decision":if answer.cancelled{"deny"}else{"allow"}});
                            if !answer.cancelled{payload["answers"]=native["answers"].clone();}
                            let request=agent_client_protocol::UntypedMessage::new("_codebuddy.ai/resolveInterruption",payload).ok();
                            let sent=bridge.as_ref().and_then(|bridge|request.and_then(|request|bridge.with_current(||connection.send_request(request))));
                            if let Some(sent)=sent {
                                let resolved=tokio::time::timeout(std::time::Duration::from_secs(15),sent.block_task()).await
                                    .is_ok_and(|result|result.is_ok_and(|value|value["resolved"]==true));
                                if !resolved && let Some(bridge)=&bridge {bridge.delivery_failed(id.clone());}
                            }
                            let _=responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
                        });
                        return Ok(agent_client_protocol::Handled::Yes);
                    }
                }
                // Surface the vendor request verbatim and park the responder;
                // the hub answers through respond_to_interaction with one of
                // the vendor option ids. No auto-approval path exists.
                let native_request_id = format!("acp-perm-{}", uuid_question_id());
                let options = map_permission_options(&request.options);
                permission_tasks
                    .pending
                    .lock()
                    .expect("pending permissions")
                    .insert(
                        native_request_id.clone(),
                        PendingPermission {
                            responder,
                            options: request.options.clone(),
                            session_id: request.session_id.to_string(),
                            tool_call_id: request.tool_call.tool_call_id.0.to_string(),
                        },
                    );
                if let Some(updates) = permission_tasks
                    .updates
                    .lock()
                    .expect("updates sink")
                    .as_ref()
                {
                    let _ = updates.send(DriverEvent::ApprovalRequested {
                        native_request_id,
                        tool_name: request.tool_call.fields.title.clone().unwrap_or_else(|| {
                            request.tool_call.tool_call_id.0.to_string()
                        }),
                        command_summary: request
                            .tool_call
                            .fields
                            .raw_input
                            .as_ref()
                            .map(std::string::ToString::to_string)
                            .or_else(|| permission_tasks.activity.lock().expect("activity")
                                .approval_input(&request.session_id.to_string(), &request.tool_call.tool_call_id.0)
                                .map(|input| input.to_string()))
                            .or_else(|| serde_json::to_string(&request.tool_call.fields).ok())
                            .unwrap_or_else(|| {
                                request.tool_call.fields.title.clone().unwrap_or_default()
                            }),
                        options,
                    });
                }
                // Ownership of the responder moved into the pending map; the
                // answer arrives asynchronously via respond_to_interaction.
                Ok(agent_client_protocol::Handled::Yes)
            },
            on_receive_request!(),
        )
        .on_receive_notification(async move |message: agent_client_protocol::UntypedMessage, connection| {
            let owner = vendor_owner.as_ref().and_then(std::sync::Weak::upgrade);
            let _gate = owner.as_ref().map(|owner| owner.lifecycle.lock().expect("ACP lifecycle"));
            if vendor_owner.is_some() && owner.as_ref().is_none_or(|owner| !owner.accepts_update(message.params["sessionId"].as_str())) { return Ok(()); }
            if message.method == "_x.ai/session_notification" {
                session_usage::observe_grok(&vendor_quota.usage, &message.params);
                let records = vendor_quota.observe_live_grok(&message.params);
                if let Some(updates) = vendor_updates.lock().expect("updates").as_ref() {
                    for request in records { let _ = updates.send(DriverEvent::UsageRecorded { request }); }
                }
                let update = message.params.get("update").unwrap_or(&message.params);
                if update["sessionUpdate"] == "response_completed" {
                    let query = vendor_quota.grok_usage.lock().expect("grok usage").cost_query(message.params["sessionId"].as_str().unwrap_or(""));
                    if let (Some(query),Some(owner))=(query,owner.as_ref())
                        && let Some(scope)=auxiliary::Call::acquire(owner,&query.session,None) {
                            let connection=connection.clone();
                            vendor_background.spawn(async move {
                                let Ok(request)=agent_client_protocol::UntypedMessage::new("_x.ai/session/usage",serde_json::json!({"sessionId":query.session})) else{return;};
                                let deadline=std::time::Instant::now()+std::time::Duration::from_secs(2);
                                let read=async {connection.send_request(request).block_task().await.map_err(|_|driver_error_from_channel())};
                                if let Ok(value)=scope.wait(read).await
                                    && std::time::Instant::now()<deadline {
                                        scope.with_current(|shared| {
                                            let total=shared.quota.grok_usage.lock().expect("grok usage").apply_cost(&query,&value);
                                            if let Some(session_cost_usd)=total {
                                                shared.quota.usage.merge(&query.session,driver_api::usage::SessionUsageSnapshot{session_cost_usd:Some(session_cost_usd),..Default::default()});
                                            }
                                        });
                                }
                            });
                    }
                }
            }
            if message.method == "_x.ai/session_notification"
                && let Some(updates)=vendor_updates.lock().expect("updates").as_ref(){
                    for event in driver_api::presentation::presentation_events("grok",&message.method,&message.params){let _=updates.send(event);}
                }
            Ok(())
        }, on_receive_notification!())
        .on_receive_request(async move |message: agent_client_protocol::UntypedMessage, responder: Responder<serde_json::Value>, _connection| {
            let provider = match message.method.as_str() {
                "elicitation/create" => Some("acp"),
                "cursor/ask_question" | "_cursor/ask_question" => Some("cursor"),
                "_x.ai/ask_user_question" => Some("grok"),
                "_codebuddy.ai/question" => Some("codebuddy"),
                _ => None,
            };
            if let Some(provider) = provider {
                let owner = question_owner.as_ref().and_then(std::sync::Weak::upgrade);
                let _gate = owner.as_ref().map(|owner| owner.lifecycle.lock().expect("ACP lifecycle"));
                let session_id = message.params["sessionId"].as_str().filter(|id| !id.is_empty());
                if question_owner.is_some() && owner.as_ref().is_none_or(|owner| owner.validate_interaction(session_id).is_err()) {
                    let cancelled = agent_domain::QuestionAnswer { cancelled: true, values: serde_json::Value::Null };
                    responder.respond(driver_api::questions::native_answer(provider, &message.params, &cancelled))?;
                    return Ok(agent_client_protocol::Handled::Yes);
                }
                let Ok(form) = driver_api::questions::native_form(provider, &message.params) else { responder.respond_with_internal_error("Unsupported question form")?; return Ok(agent_client_protocol::Handled::Yes); };
                let request_id=format!("acp-question-{}", uuid_question_id());
                let session_id=message.params["sessionId"].as_str().unwrap_or("").to_owned();
                let (reply, receiver)=oneshot::channel();
                questions.lock().expect("questions").insert(request_id.clone(),PendingQuestion{session_id,form:form.clone(),reply});
                let delivered=question_updates.lock().expect("updates").as_ref().is_some_and(|updates|updates.send(DriverEvent::QuestionRequested{native_request_id:request_id.clone(),form}).is_ok());
                if !delivered { questions.lock().expect("questions").remove(&request_id); }
                let pending=Arc::clone(&questions);
                question_background.spawn(async move {
                    let answer=receiver.await.unwrap_or(agent_domain::QuestionAnswer{cancelled:true,values:serde_json::Value::Null});
                    pending.lock().expect("questions").remove(&request_id);
                    let _=responder.respond(driver_api::questions::native_answer(provider,&message.params,&answer));
                });
                return Ok(agent_client_protocol::Handled::Yes);
            }
            if message.method != "cursor/task" { responder.respond_with_internal_error("Unsupported client method")?; return Ok(agent_client_protocol::Handled::Yes); }
            let owner=cursor_owner.as_ref().and_then(std::sync::Weak::upgrade);
            let _gate=owner.as_ref().map(|owner|owner.lifecycle.lock().expect("ACP lifecycle"));
            if cursor_owner.is_some() && owner.as_ref().is_none_or(|owner|!owner.accepts_update(message.params["sessionId"].as_str())) {
                responder.respond(serde_json::json!({"outcome":{"outcome":"rejected","reason":"Task session is not active"}}))?;
                return Ok(agent_client_protocol::Handled::Yes);
            }
            let (reply, event) = cursor_activity.lock().expect("activity").cursor_task(&message.params);
            if let (Some(updates), Some(event)) = (cursor_updates.lock().expect("updates").as_ref(), event) { let _ = updates.send(event); }
            responder.respond(reply)?;
            Ok(agent_client_protocol::Handled::Yes)
        }, on_receive_request!())
        .connect_with(transport, async move |connection| {
            let initialized = tokio::time::timeout(std::time::Duration::from_secs(45), initialize(&connection))
                .await.map_err(|_| AcpError::internal_error())??;
            if let Some(owner) = command_owner.as_ref().and_then(std::sync::Weak::upgrade) {
                owner.native_close.store(initialized.agent_capabilities.session_capabilities.close.is_some(), Ordering::Release);
            }
            let mut initial_models = serde_json::to_value(&initialized).unwrap_or_default();
            if models::has_kiro_template(&initial_models) {
                let template = models::kiro_template(&connection).await.map_err(|_| AcpError::internal_error())?;
                initial_models["configOptions"] = template["configOptions"].clone();
            }
            let multi_root = initialized.agent_capabilities.session_capabilities.additional_directories.is_some();
            loop {
                tokio::select! {
                    command = commands.recv() => {
                        let Some(command) = command else { break };
                        if matches!(command, Command::CloseSession { .. } | Command::Prompt { .. } | Command::ReadGrokBilling { .. } | Command::ReadKiroUsage { .. } | Command::ReadKiroContext { .. }) {
                            // Prompts run on their own task so session/cancel
                            // stays responsive while a turn is in flight.
                            command_background.spawn(handle_command(connection.clone(), command, multi_root, initial_models.clone(), Arc::clone(&session_states), Arc::clone(&command_quota)));
                        } else {
                            handle_command(connection.clone(), command, multi_root, initial_models.clone(), Arc::clone(&session_states), Arc::clone(&command_quota)).await;
                        }
                    }
                    cancel = cancels.recv() => {
                        let Some(turn) = cancel else { break };
                        turn.cancel_on(&connection)?;
                    }
                }
            }
            Ok(())
        })
        .await;
    if let Err(_error) = result {
        // The connection is gone; command senders observe this as a closed
        // channel and the driver reports Disconnected on the next use.
    }
}

fn additional_roots(multi_root: bool, directories: Vec<PathBuf>) -> Vec<PathBuf> {
    if multi_root { directories } else { vec![] }
}

#[cfg(test)]
mod multi_root_tests {
    use super::*;
    #[test]
    fn negotiated_roots_apply_to_both_new_and_load_and_are_omitted_without_capability() {
        for supported in [false, true] {
            let roots = additional_roots(supported, vec![PathBuf::from("/extra folder")]);
            let new = serde_json::to_value(
                NewSessionRequest::new("/primary").additional_directories(roots.clone()),
            )
            .unwrap();
            let load = serde_json::to_value(
                LoadSessionRequest::new(SessionId::new("session"), "/primary")
                    .additional_directories(roots),
            )
            .unwrap();
            for request in [new, load] {
                assert_eq!(request["cwd"], "/primary");
                if supported {
                    assert_eq!(
                        request["additionalDirectories"],
                        serde_json::json!(["/extra folder"])
                    );
                } else {
                    assert!(request.get("additionalDirectories").is_none());
                }
            }
        }
    }
}

async fn handle_command(
    connection: ConnectionTo<Agent>,
    command: Command,
    multi_root: bool,
    initial_models: serde_json::Value,
    session_states: Arc<std::sync::Mutex<session_state::SessionStates>>,
    quota: Arc<AcpQuotaState>,
) {
    match command {
        Command::CloseSession { session_id, reply } => {
            let result: Result<CloseSessionResponse, _> = connection
                .send_request(CloseSessionRequest::new(SessionId::new(session_id)))
                .block_task()
                .await;
            let _ =
                reply.send(result.map(|_| ()).map_err(|error| {
                    map_session_error(&error, "agent rejected session/close", None)
                }));
        }
        Command::ForgetSession { session_id, reply } => {
            session_states
                .lock()
                .expect("session states")
                .forget(&session_id);
            let _ = reply.send(());
        }
        Command::ProbeReady { mut reply } => {
            let pending = async {
                let initialized: InitializeResponse =
                    serde_json::from_value(initial_models.clone()).map_err(|_| {
                        DriverError::Protocol("invalid ACP initialization response".into())
                    })?;
                if initialized.protocol_version != ProtocolVersion::V1 {
                    return Err(DriverError::Protocol(
                        "agent negotiated an unsupported ACP protocol version".into(),
                    ));
                }
                let directory = tempfile::tempdir().map_err(|_| {
                    DriverError::Protocol("cannot create ACP probe directory".into())
                })?;
                let state = models::session_request(
                    &connection,
                    "session/new",
                    serde_json::json!({"cwd":directory.path(),"mcpServers":[]}),
                    None,
                )
                .await?;
                // Match the typed probe's required session-id validation. This
                // session is not entered into task mappings or reusable bindings.
                if state["sessionId"].as_str().is_none_or(str::is_empty) {
                    return Err(DriverError::Protocol("ACP session id missing".into()));
                }
                Ok(ProbeReport {
                    resume_capability: if supports_native_resume(&initial_models) {
                        SessionResumeCapability::Supported
                    } else {
                        SessionResumeCapability::Unsupported
                    },
                    native_version: initialized.agent_info.map(|info| info.version),
                })
            };
            let result = tokio::select! {
                biased;
                () = reply.closed() => return,
                result = pending => result,
            };
            let _ = reply.send(result);
        }
        Command::PermissionCatalog { workspace, reply } => {
            if reply.is_closed() {
                return;
            }
            let result=async {
                let directory=tempfile::tempdir().map_err(|_|DriverError::Protocol("无法创建权限探测目录".into()))?;
                let state=models::session_request(&connection,"session/new",serde_json::json!({"cwd":workspace.as_deref().unwrap_or(directory.path()),"mcpServers":[]}),Some(&quota.stderr_tail)).await?;
                Ok(permissions::catalog(&state))
            }.await;
            let _ = reply.send(result);
        }
        Command::Catalog {
            model,
            workspace,
            reply,
        } => {
            if reply.is_closed() {
                return;
            }
            let initial = models::catalog(&initial_models);
            let result = if model.is_none() && workspace.is_none() && !initial.models.is_empty() {
                Ok(initial)
            } else {
                async {
                    let directory = tempfile::tempdir().map_err(|_| DriverError::Protocol("无法创建模型探测工作区".into()))?;
                    let mut state = models::session_request(&connection,"session/new",serde_json::json!({"cwd":workspace.as_deref().unwrap_or(directory.path()),"mcpServers":[]}),Some(&quota.stderr_tail)).await?;
                    let known = models::catalog(&state);
                    if let Some(model) = model {
                        // A removed selection must not prevent refreshing the remaining catalog.
                        if !known.models.iter().any(|m| m.id == model) { return Ok(known); }
                        if let Some(config) = models::model_config(&state) {
                            let response = models::raw(&connection,"session/set_config_option",serde_json::json!({"sessionId":state["sessionId"],"configId":config["id"],"value":model})).await?;
                            state = session_state::merge(&state, &response);
                        }
                    }
                    let mut catalog = models::catalog(&state);
                    // Probing another model's effort options does not change the native default.
                    catalog.default_model = known.default_model;
                    Ok(catalog)
                }.await
            };
            let _ = reply.send(result);
        }
        Command::NewSession {
            history_mcp,
            grok,
            permission_mode,
            model,
            thinking,
            cwd,
            additional_directories,
            reply,
        } => {
            let mut params = serde_json::to_value(
                NewSessionRequest::new(cwd.clone())
                    .additional_directories(additional_roots(multi_root, additional_directories)),
            )
            .unwrap_or_default();
            params["mcpServers"] = driver_api::HistoryMcp::native_servers(history_mcp.as_ref());
            if grok {
                permissions::meta(
                    &mut params,
                    Some(permission_mode.as_deref().unwrap_or("ask")),
                );
            }
            let result = async {
                let state = models::session_request(
                    &connection,
                    "session/new",
                    params.clone(),
                    Some(&quota.stderr_tail),
                )
                .await?;
                let id = state["sessionId"]
                    .as_str()
                    .ok_or_else(|| DriverError::Protocol("ACP session id missing".into()))?;
                let session_id = SessionId::new(id);
                let state = session_states
                    .lock()
                    .expect("session states")
                    .new_session(id, state.clone());
                let revision = session_states
                    .lock()
                    .expect("session states")
                    .begin(&session_id.to_string());
                let configured = async {
                    let state = models::select(
                        &connection,
                        &state,
                        &session_id,
                        model.as_deref(),
                        thinking.as_deref(),
                    )
                    .await?;
                    let state = permissions::select(
                        &connection,
                        &state,
                        &session_id,
                        permission_mode.as_deref(),
                    )
                    .await?;
                    let state = session_states
                        .lock()
                        .expect("session states")
                        .finish_setup(&session_id.to_string(), revision, state, params)
                        .ok_or_else(|| {
                            DriverError::Protocol("Session setup was superseded".into())
                        })?;
                    quota.observe_model(&session_id.to_string(), &state);
                    Ok(session_id.clone())
                }
                .await;
                if configured.is_err() {
                    session_states
                        .lock()
                        .expect("session states")
                        .fail(&session_id.to_string(), revision);
                }
                configured
            }
            .await;
            if let Ok(id) = &result {
                quota
                    .directories
                    .lock()
                    .unwrap()
                    .insert(id.to_string(), cwd);
                if grok {
                    // A newly created native session has no earlier conversation cost.
                    quota.usage.merge(
                        &id.to_string(),
                        driver_api::usage::SessionUsageSnapshot {
                            session_cost_usd: Some(0.0),
                            ..driver_api::usage::SessionUsageSnapshot::new()
                        },
                    );
                }
            }
            let _ = reply.send(result);
        }
        Command::LoadSession {
            history_mcp,
            grok,
            permission_mode,
            model,
            thinking,
            session_id,
            cwd,
            additional_directories,
            reply,
        } => {
            let mut params = serde_json::to_value(
                LoadSessionRequest::new(session_id.clone(), cwd.clone())
                    .additional_directories(additional_roots(multi_root, additional_directories)),
            )
            .unwrap_or_default();
            params["mcpServers"] = driver_api::HistoryMcp::native_servers(history_mcp.as_ref());
            if grok {
                permissions::meta(
                    &mut params,
                    Some(permission_mode.as_deref().unwrap_or("ask")),
                );
            }
            let resident = session_states
                .lock()
                .expect("session states")
                .resident(&session_id.to_string(), &params);
            let mut revision = session_states
                .lock()
                .expect("session states")
                .begin(&session_id.to_string());
            let result = async {
                let state = if let Some(state) = resident {
                    state
                } else {
                    models::session_request(
                        &connection,
                        resume_method(&initial_models),
                        params.clone(),
                        Some(&quota.stderr_tail),
                    )
                    .await?
                };
                let state = session_states
                    .lock()
                    .expect("session states")
                    .finish(&session_id.to_string(), revision, state)
                    .ok_or_else(|| {
                        DriverError::Protocol("Session restore was superseded".into())
                    })?;
                revision = session_states
                    .lock()
                    .expect("session states")
                    .begin(&session_id.to_string());
                let state = models::select(
                    &connection,
                    &state,
                    &session_id,
                    model.as_deref(),
                    thinking.as_deref(),
                )
                .await?;
                let state = permissions::select(
                    &connection,
                    &state,
                    &session_id,
                    permission_mode.as_deref(),
                )
                .await?;
                let state = session_states
                    .lock()
                    .expect("session states")
                    .finish_setup(&session_id.to_string(), revision, state, params)
                    .ok_or_else(|| {
                        DriverError::Protocol("Session configuration was superseded".into())
                    })?;
                quota.observe_model(&session_id.to_string(), &state);
                Ok(session_id.clone())
            }
            .await;
            if result.is_err() {
                session_states
                    .lock()
                    .expect("session states")
                    .fail(&session_id.to_string(), revision);
            }
            if let Ok(id) = &result {
                quota
                    .directories
                    .lock()
                    .unwrap()
                    .insert(id.to_string(), cwd);
            }
            let _ = reply.send(result);
        }

        Command::Prompt {
            session_id,
            blocks,
            image_compatibility,
            reply,
            turn,
        } => {
            if blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::Image(_)))
                && !supports_prompt_images(&initial_models, image_compatibility)
            {
                let _ = reply.send(Err(DriverError::CapabilityUnavailable(
                    "当前 Agent 未声明支持图片附件，请更新 Agent 后重试".into(),
                )));
                return;
            }
            // Keep vendor usage extensions before deserializing the standard response.
            let Ok(request) = agent_client_protocol::UntypedMessage::new(
                "session/prompt",
                serde_json::to_value(PromptRequest::new(session_id.clone(), blocks))
                    .unwrap_or_default(),
            ) else {
                let _ = reply.send(Err(driver_error_from_channel()));
                return;
            };
            let stderr = quota.stderr_tail.begin_request();
            let pending = if let Some(turn) = &turn {
                let mut dispatch = turn.dispatch.lock().expect("ACP prompt dispatch");
                if dispatch.cancel_requested {
                    let _ = reply.send(Ok(StopReason::Cancelled));
                    return;
                }
                let pending = connection.send_request(request);
                dispatch.sent = true;
                pending
            } else {
                connection.send_request(request)
            };
            let response = if let Some(turn) = &turn {
                tokio::select! {
                    biased;
                    () = turn.closed_natively() => {
                        // session/close confirms all work was cancelled even
                        // when the agent never answers the old prompt request.
                        let _ = reply.send(Ok(StopReason::Cancelled));
                        return;
                    }
                    response = pending.block_task() => response,
                }
            } else {
                pending.block_task().await
            };
            if let Ok(done) = &response {
                let raw = serde_json::to_value(done).unwrap_or_default();
                quota.usage.merge(
                    &session_id.to_string(),
                    driver_api::usage::acp(&raw["usage"]),
                );
                let records = quota.settle_live_grok(
                    &session_id.to_string(),
                    raw["_meta"]["promptId"].as_str(),
                    &raw["_meta"]["usage"],
                );
                // Settlement notifications usually precede this response. The fallback preserves
                // native prompt identity if a CLI only returns it on the prompt response.
                for request in records {
                    quota
                        .usage
                        .observe_request(&session_id.to_string(), request);
                }
            }
            let result = response
                .map_err(|error| {
                    map_session_error(
                        &error,
                        "agent rejected the ACP session/prompt request",
                        Some(&stderr),
                    )
                })
                .and_then(|raw| {
                    serde_json::from_value(raw)
                        .map_err(|_| DriverError::Protocol("invalid ACP prompt response".into()))
                })
                .and_then(prompt_stop_reason);
            if result.is_err() {
                session_states
                    .lock()
                    .expect("session states")
                    .invalidate(&session_id.to_string());
            }
            let _ = reply.send(result);
        }
        Command::ReadKiroContext { scope, reply } => {
            if reply.is_closed() || scope.with_current(|_| ()).is_none() {
                return;
            }
            let read = async {
                let request = agent_client_protocol::UntypedMessage::new(
                    "_kiro/session/context",
                    serde_json::json!({"sessionId":scope.session(),"subcommand":"show"}),
                )
                .map_err(|_| driver_error_from_channel())?;
                connection
                    .send_request(request)
                    .block_task()
                    .await
                    .map_err(|_| driver_error_from_channel())
            };
            // Once sent, caller cancellation cannot drop native ownership.
            let result = scope.wait(read).await;
            let _ = reply.send(result);
        }
        Command::ReadKiroUsage { mut reply } => {
            // Account query needs no session and must not wait for a model turn.
            let read = kiro_quota::read(&connection);
            tokio::select! {
                result = read => { let _ = reply.send(result); }
                () = reply.closed() => {}
            }
        }
        Command::ReadGrokBilling { reply } => {
            // Vendor extension methods ride the [ext] enum variants; the wire
            // method keeps its leading underscore while the ExtRequest struct
            // stores the stripped name.
            let params: std::sync::Arc<serde_json::value::RawValue> =
                serde_json::value::to_raw_value(&serde_json::json!({}))
                    .expect("empty Grok billing params serialize")
                    .into();
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(8),
                connection
                    .send_request(ClientRequest::ExtMethodRequest(ExtRequest::new(
                        "_x.ai/billing",
                        params,
                    )))
                    .block_task(),
            )
            .await;
            // ClientRequest's response type is the raw response JSON value.
            let result = match response {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(DriverError::Protocol(format!(
                    "Grok billing extension failed (code {})",
                    error.code
                ))),
                Err(_) => Err(DriverError::Disconnected(
                    "Grok billing request timed out".into(),
                )),
            };
            let _ = reply.send(result);
        }
    }
}

/// Preserves the vendor option list verbatim; semantic roles come only from
/// the protocol's option kind, never from label text.
fn map_permission_options(options: &[PermissionOption]) -> Vec<ApprovalOption> {
    options
        .iter()
        .map(|option| ApprovalOption {
            option_id: option.option_id.0.to_string(),
            label: option.name.clone(),
            semantic_role: match option.kind {
                PermissionOptionKind::AllowOnce => ApprovalSemanticRole::ApproveOnce,
                PermissionOptionKind::AllowAlways => ApprovalSemanticRole::ApprovePersistent,
                PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways => {
                    ApprovalSemanticRole::Reject
                }
                _ => ApprovalSemanticRole::Other,
            },
        })
        .collect()
}

fn map_session_update(notification: &SessionNotification) -> Vec<DriverEvent> {
    let mut events = Vec::new();
    match &notification.update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            events.push(map_content_chunk(chunk));
        }
        SessionUpdate::ToolCall(call) => events.push(DriverEvent::ToolStarted {
            item_id: call.tool_call_id.0.to_string(),
            tool_name: call.title.clone(),
            summary: format!("正在使用 {}", call.title),
        }),
        SessionUpdate::ToolCallUpdate(update) => {
            if let Some(event) = map_tool_update(
                update.fields.status,
                update.fields.title.as_deref(),
                &update.tool_call_id,
            ) {
                events.push(event);
            }
        }
        // UserMessageChunk echoes the user's own prompt; AgentThoughtChunk is
        // reasoning with unverified visibility and is deliberately not recorded.
        SessionUpdate::UserMessageChunk(_) | SessionUpdate::AgentThoughtChunk(_) => {}
        other => events.push(DriverEvent::VendorExtension {
            method: "session/update".into(),
            payload: serde_json::to_value(update_name(other)).unwrap_or_default(),
        }),
    }
    events
}

fn map_content_chunk(chunk: &ContentChunk) -> DriverEvent {
    match &chunk.content {
        ContentBlock::Text(text) => DriverEvent::TextDelta {
            item_id: "message".into(),
            text: text.text.clone(),
        },
        _ => DriverEvent::ProtocolWarning {
            message: "a non-text content block was omitted from the timeline".into(),
        },
    }
}

fn map_tool_update(
    status: Option<ToolCallStatus>,
    title: Option<&str>,
    tool_call_id: &agent_client_protocol::schema::v1::ToolCallId,
) -> Option<DriverEvent> {
    let name = title.map_or_else(|| tool_call_id.0.to_string(), str::to_owned);
    match status {
        Some(ToolCallStatus::Completed) => Some(DriverEvent::ToolCompleted {
            item_id: tool_call_id.0.to_string(),
            tool_name: name,
            summary: "工具调用已完成".into(),
            success: true,
        }),
        Some(ToolCallStatus::Failed) => Some(DriverEvent::ToolCompleted {
            item_id: tool_call_id.0.to_string(),
            tool_name: name,
            summary: "工具调用失败".into(),
            success: false,
        }),
        Some(ToolCallStatus::Pending | ToolCallStatus::InProgress | _) | None => None,
    }
}

fn update_name(update: &SessionUpdate) -> &'static str {
    match update {
        SessionUpdate::UserMessageChunk(_) => "user_message_chunk",
        SessionUpdate::AgentMessageChunk(_) => "agent_message_chunk",
        SessionUpdate::AgentThoughtChunk(_) => "agent_thought_chunk",
        SessionUpdate::ToolCall(_) => "tool_call",
        SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
        SessionUpdate::Plan(_) => "plan",
        SessionUpdate::AvailableCommandsUpdate(_) => "available_commands_update",
        SessionUpdate::CurrentModeUpdate(_) => "current_mode_update",
        SessionUpdate::ConfigOptionUpdate(_) => "config_option_update",
        SessionUpdate::SessionInfoUpdate(_) => "session_info_update",
        SessionUpdate::UsageUpdate(_) => "usage_update",
        _ => "other",
    }
}

// Grok 1.0.30 accepts Image blocks, but omits promptCapabilities.image.
// Keep this verified exception narrow; unknown agents must honor negotiation.
fn grok_image_compatibility(provider: &str, version: Option<&str>) -> bool {
    provider == "grok" && version.is_some_and(|v| v.split_whitespace().any(|part| part == "1.0.30"))
}
fn supports_prompt_images(initial: &serde_json::Value, compatibility: bool) -> bool {
    initial["agentCapabilities"]["promptCapabilities"]["image"] == true || compatibility
}
async fn attachment_blocks(
    provider: &str,
    items: &[agent_domain::PromptAttachment],
) -> Result<Vec<ContentBlock>, DriverError> {
    let mut blocks = Vec::new();
    for item in items {
        let value = if driver_api::attachments::is_image(item) {
            serde_json::json!({"type":"image","mimeType":item.mime_type,"data":driver_api::attachments::image_data(item).await?})
        } else {
            let uri = url::Url::from_file_path(&item.path)
                .map_err(|()| DriverError::CapabilityUnavailable("附件路径无效".into()))?;
            // CodeBuddy uses name before decoding uri into its native @file mention.
            // A basename would resolve against cwd and lose an out-of-workspace attachment.
            let name = if provider == "codebuddy" {
                &item.path
            } else {
                &item.name
            };
            serde_json::json!({"type":"resource_link","uri":uri.as_str(),"name":name,"mimeType":item.mime_type})
        };
        blocks.push(
            serde_json::from_value(value)
                .map_err(|_| DriverError::Protocol("附件内容编码失败".into()))?,
        );
    }
    Ok(blocks)
}

fn context_block(delivery: &ContextDelivery) -> ContentBlock {
    let kind = match delivery.kind {
        ContextDeliveryKind::Handoff => "Handoff",
        ContextDeliveryKind::Reconciliation => "Reconciliation",
    };
    let body = String::from_utf8_lossy(&delivery.content);
    ContentBlock::Text(TextContent::new(format!(
        "[dogcodes {kind} {} sha256={}]\n{body}",
        delivery.artifact_id, delivery.payload_sha256
    )))
}

#[async_trait]
impl AgentDriver for AcpDriver {
    fn independent_session_driver(&self) -> Result<Option<Arc<dyn AgentDriver>>, DriverError> {
        let driver = Self::new(
            self.executor.provider_id.as_str(),
            self.executor.installation_id.as_deref().unwrap_or(""),
            self.shared.launch.clone(),
            self.native_version.clone(),
            self.resume_capability,
        )
        .map_err(|_| DriverError::Protocol("cannot recreate ACP installation".into()))?;
        Ok(Some(Arc::new(driver)))
    }
    async fn permission_catalog(
        &self,
        workspace: Option<&str>,
    ) -> Result<driver_api::PermissionCatalog, DriverError> {
        if self.executor.provider_id.as_str() == "cursor" {
            return Ok(permissions::cursor_catalog());
        }
        let workspace = workspace.map(str::to_owned);
        let scope_workspace = workspace.clone();
        let mut catalog = self
            .metadata(scope_workspace, move |reply| Command::PermissionCatalog {
                workspace: workspace.map(Into::into),
                reply,
            })
            .await?;
        if self.executor.provider_id.as_str() == "kimi" {
            permissions::kimi_labels(&mut catalog);
        }
        Ok(catalog)
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
        // CodeBuddy's normal turns can reuse the process. Cancelled/failed
        // turns mark it for replacement before the next session/load.
        self.executor.provider_id.as_str() != "codebuddy"
            || self.resume_capability == SessionResumeCapability::Supported
    }

    fn resume_capability(&self) -> SessionResumeCapability {
        self.resume_capability
    }

    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        self.create_selected_session(request, None, None).await
    }
    async fn model_catalog(&self) -> Result<driver_api::ModelCatalog, DriverError> {
        self.model_catalog_for(None).await
    }
    async fn model_catalog_for(
        &self,
        model: Option<&str>,
    ) -> Result<driver_api::ModelCatalog, DriverError> {
        self.model_catalog_in_workspace(model, None).await
    }
    async fn model_catalog_in_workspace(
        &self,
        model: Option<&str>,
        workspace: Option<&str>,
    ) -> Result<driver_api::ModelCatalog, DriverError> {
        let model = model.map(str::to_owned);
        let workspace = workspace.map(str::to_owned);
        let scope_workspace = workspace.clone();
        let mut catalog = self
            .metadata(scope_workspace, move |reply| Command::Catalog {
                model,
                workspace: workspace.map(Into::into),
                reply,
            })
            .await?;
        if self.executor.provider_id.as_str() == "opencode" {
            opencode_models::filter_retired(&mut catalog);
        }
        Ok(catalog)
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        let permit = self.shared.setup.clone().lock_owned().await;
        self.no_pending_stop()?;
        self.shared.setup_available()?;
        let connection = self
            .ensure_workspace_connection(
                if self.executor.provider_id.as_str() == "cursor" {
                    Some(request.permission_mode.as_deref().unwrap_or("default"))
                } else {
                    None
                },
                Some(&request.working_directory),
            )
            .await?;
        let (reply_tx, reply_rx) = oneshot::channel();
        let command = Command::NewSession {
            history_mcp: request.history_mcp.clone(),
            grok: self.executor.provider_id.as_str() == "grok",
            permission_mode: if self.executor.provider_id.as_str() == "cursor" {
                None
            } else {
                request.permission_mode.clone()
            },
            model: model.map(Into::into),
            thinking: thinking.map(Into::into),
            cwd: PathBuf::from(&request.working_directory),
            additional_directories: request
                .additional_directories
                .iter()
                .map(PathBuf::from)
                .collect(),
            reply: reply_tx,
        };
        self.configured_session(connection, command, reply_rx, permit)
            .await
    }

    async fn resume_session(
        &self,
        native_session_id: &str,
        working_directory: &str,
    ) -> Result<SessionHandle, DriverError> {
        self.resume_workspace_session(
            native_session_id,
            SessionRequest {
                history_mcp: None,
                permission_mode: None,
                working_directory: working_directory.into(),
                additional_directories: vec![],
            },
            None,
        )
        .await
    }

    async fn resume_workspace_session(
        &self,
        native_session_id: &str,
        workspace: SessionRequest,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.resume_selected_session(native_session_id, workspace, model, None)
            .await
    }
    async fn resume_selected_session(
        &self,
        native_session_id: &str,
        workspace: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        let permit = self.shared.setup.clone().lock_owned().await;
        self.no_pending_stop()?;
        self.shared.setup_available()?;
        if self
            .shared
            .sessions
            .lock()
            .expect("ACP sessions")
            .get(native_session_id)
            .is_some_and(|resident| resident.is_closing())
        {
            return Err(DriverError::CapabilityUnavailable(
                "ACP session retirement is still pending".into(),
            ));
        }
        if self.resume_capability != SessionResumeCapability::Supported {
            return Err(DriverError::CapabilityUnavailable(
                "this agent did not advertise ACP session/load".into(),
            ));
        }
        // Reuse a fully initialized session on the current connection when its
        // workspace/MCP binding matches; otherwise restore with session/load.
        let cwd = PathBuf::from(&workspace.working_directory);
        let connection = self
            .ensure_workspace_connection(
                if self.executor.provider_id.as_str() == "cursor" {
                    Some(workspace.permission_mode.as_deref().unwrap_or("default"))
                } else {
                    None
                },
                Some(&workspace.working_directory),
            )
            .await?;
        let (reply_tx, reply_rx) = oneshot::channel();
        let command = Command::LoadSession {
            history_mcp: workspace.history_mcp.clone(),
            grok: self.executor.provider_id.as_str() == "grok",
            permission_mode: if self.executor.provider_id.as_str() == "cursor" {
                None
            } else {
                workspace.permission_mode.clone()
            },
            model: model.map(Into::into),
            thinking: thinking.map(Into::into),
            session_id: SessionId::new(native_session_id),
            cwd,
            additional_directories: workspace
                .additional_directories
                .iter()
                .map(PathBuf::from)
                .collect(),
            reply: reply_tx,
        };
        self.configured_session(connection, command, reply_rx, permit)
            .await
    }

    async fn current_session_handle(&self, native_session_id: &str) -> Option<SessionHandle> {
        let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
        if self.shared.reconnect_required.load(Ordering::Acquire) {
            return None;
        }
        self.shared
            .sessions
            .lock()
            .expect("ACP sessions")
            .get(native_session_id)
            .filter(|r| !r.is_closing() && !r.epoch.load(Ordering::Acquire))
            .map(|r| r.handle.clone())
    }

    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: HubPromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        let _setup = self.shared.setup.lock().await;
        {
            let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
            self.shared.validate_handle(handle)?;
        }
        if !self
            .shared
            .sessions
            .lock()
            .expect("ACP sessions")
            .contains_key(&handle.native_session_id)
        {
            return Err(DriverError::SessionMissing(
                "Create or restore the ACP session before sending".into(),
            ));
        }
        if self
            .shared
            .closed_sessions
            .lock()
            .expect("closed sessions")
            .contains(&handle.native_session_id)
        {
            return Err(DriverError::SessionMissing(
                "ACP session was closed; restore it before sending".into(),
            ));
        }
        let attachment_blocks =
            attachment_blocks(self.executor.provider_id.as_str(), &request.attachments).await?;
        let connection = self
            .shared
            .connection
            .lock()
            .await
            .as_ref()
            .filter(|c| !c.unavailable())
            .cloned()
            .ok_or_else(driver_error_from_channel)?;
        let (current_turn, completed) = {
            let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
            self.shared.validate_handle(handle)?;
            self.no_pending_stop()?;
            self.shared.prompt_available()?;
            if connection.unavailable() {
                return Err(driver_error_from_channel());
            }
            let resident = {
                let sessions = self.shared.sessions.lock().expect("ACP sessions");
                sessions
                    .get(&handle.native_session_id)
                    .cloned()
                    .ok_or_else(|| DriverError::SessionMissing("ACP session was retired".into()))?
            };
            if !Arc::ptr_eq(&resident.epoch, &connection.finished) || resident.is_closing() {
                return Err(DriverError::SessionIncompatible(
                    "Restore the ACP session on the current connection before sending".into(),
                ));
            }
            if self
                .shared
                .latest_turn
                .lock()
                .expect("ACP turn")
                .as_ref()
                .is_some_and(|turn| {
                    turn.failed_stop()
                        || (turn.session == handle.native_session_id
                            && !Arc::ptr_eq(&turn.epoch, &connection.finished))
                })
            {
                return Err(DriverError::SessionIncompatible(
                    "Restore the ACP session after its original connection stopped".into(),
                ));
            }
            if self
                .shared
                .closed_sessions
                .lock()
                .expect("closed sessions")
                .contains(&handle.native_session_id)
            {
                return Err(DriverError::SessionMissing(
                    "ACP session was closed; restore it before sending".into(),
                ));
            }
            if self
                .shared
                .turn_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(DriverError::CapabilityUnavailable(
                    "this ACP connection already has an active turn".into(),
                ));
            }
            *self.shared.active_session.lock().expect("active session") =
                Some(handle.native_session_id.clone());
            self.shared
                .approval_rejected
                .store(false, Ordering::Release);
            self.shared
                .interrupt_requested
                .store(false, Ordering::Release);
            let (turn, completed) = turn::Turn::new(
                handle.native_session_id.clone(),
                connection.finished.clone(),
                connection.native.clone(),
                connection.cancels.clone(),
                Arc::downgrade(&self.shared),
            );
            *resident.turn.lock().expect("ACP session turn") = Some(turn.clone());
            *self.shared.latest_turn.lock().expect("ACP turn") = Some(turn.clone());
            (turn, completed)
        };
        self.shared
            .activity
            .lock()
            .expect("activity")
            .begin_turn(&handle.native_session_id);
        if self.executor.provider_id.as_str() == "grok" {
            let before = self.shared.quota.usage.get(Some(&handle.native_session_id));
            self.shared
                .quota
                .grok_usage
                .lock()
                .expect("grok usage")
                .begin(&handle.native_session_id, before.as_ref());
        }
        let mut blocks = Vec::with_capacity(request.context.len() + 1);
        // Port contract: every reviewed context item is delivered, in order,
        // before the user prompt inside the same native turn.
        for delivery in &request.context {
            blocks.push(context_block(delivery));
        }
        blocks.extend(attachment_blocks);
        if !request.prompt.is_empty() {
            blocks.push(ContentBlock::Text(TextContent::new(request.prompt.clone())));
        }

        let (event_tx, event_rx) = mpsc::unbounded_channel();
        {
            let mut sink = self.shared.updates.lock().expect("updates sink");
            *sink = Some(event_tx.clone());
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        if connection
            .commands
            .send(Command::Prompt {
                turn: Some(current_turn.clone()),
                session_id: SessionId::new(handle.native_session_id.as_str()),
                blocks,
                image_compatibility: grok_image_compatibility(
                    self.executor.provider_id.as_str(),
                    self.native_version.as_deref(),
                ),
                reply: reply_tx,
            })
            .is_err()
        {
            let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
            if let Some(sink) = self.shared.updates.lock().expect("updates sink").take() {
                let _ = sink.send(DriverEvent::TurnFailed {
                    reason: "ACP agent connection is closed".into(),
                });
            }
            self.shared.turn_active.store(false, Ordering::Release);
            self.shared
                .active_session
                .lock()
                .expect("active session")
                .take();
            self.shared
                .reconnect_required
                .store(true, Ordering::Release);
            current_turn.finish(&completed, Err(driver_error_from_channel()));
            return Err(driver_error_from_channel());
        }

        // The terminal event is appended once the prompt response arrives; the
        // stream ends when this task drops its sender.
        let shared = Arc::clone(&self.shared);
        let is_grok = self.executor.provider_id.as_str() == "grok";
        let is_codebuddy = self.executor.provider_id.as_str() == "codebuddy";
        let native_session = handle.native_session_id.clone();
        tokio::spawn(async move {
            let result = reply_rx
                .await
                .unwrap_or_else(|_| Err(driver_error_from_channel()));
            let _gate = shared.lifecycle.lock().expect("ACP lifecycle");
            let settled = result.as_ref().map(|_| ()).map_err(Clone::clone);
            let retiring = current_turn.retiring.load(Ordering::Acquire);
            if result.is_err() && !retiring {
                shared.reconnect_required.store(true, Ordering::Release);
            }
            if is_codebuddy
                && !retiring
                && (!matches!(result, Ok(StopReason::EndTurn))
                    || shared.interrupt_requested.load(Ordering::Acquire))
            {
                // CodeBuddy keeps native cancellation state. Retire only the
                // affected connection, even if cancellation raced EndTurn.
                shared.reconnect_required.store(true, Ordering::Release);
            }
            let terminal = prompt_terminal(
                result,
                is_grok,
                shared.approval_rejected.load(Ordering::Acquire),
                shared.interrupt_requested.load(Ordering::Acquire),
            );
            if is_grok {
                let records = shared
                    .quota
                    .grok_usage
                    .lock()
                    .expect("grok usage")
                    .finish(&native_session);
                if let Some(updates) = shared.updates.lock().expect("updates").as_ref() {
                    // Include response-only settlement before the terminal event, retaining request IDs.
                    for request in records {
                        let _ = updates.send(DriverEvent::UsageRecorded { request });
                    }
                }
            }
            shared.questions.lock().expect("questions").clear();
            shared
                .pending_permissions
                .lock()
                .expect("pending permissions")
                .clear();
            let sink = shared.updates.lock().expect("updates sink").take();
            current_turn.finish(&completed, settled);
            shared.turn_active.store(false, Ordering::Release);
            shared.active_session.lock().expect("active session").take();
            if let Some(sink) = sink {
                for event in shared.activity.lock().expect("activity").finish() {
                    let _ = sink.send(event);
                }
                let _ = sink.send(terminal);
                // Dropping the last sender closes the receiver stream.
                drop(sink);
            }
        });
        let opencode = self.executor.provider_id.as_str() == "opencode";
        let mut observed_progress = false;
        let stream: DriverEventStream = Box::pin(UnboundedReceiverStream::new(event_rx).map(move |event| {
            observed_progress |= match &event {
                DriverEvent::TextDelta { text, .. } => !text.is_empty(),
                DriverEvent::Presentation { data, .. } => data["type"] == "tool"
                    || data["textDelta"].as_str().is_some_and(|s| !s.is_empty())
                    || data["delta"].as_str().is_some_and(|s| !s.is_empty()),
                DriverEvent::ToolStarted { .. } | DriverEvent::ApprovalRequested { .. } | DriverEvent::QuestionRequested { .. } => true,
                _ => false,
            };
            Ok(if opencode && !observed_progress && matches!(event, DriverEvent::TurnCompleted { .. }) {
                // OpenCode 1.15 can swallow native startup/database errors and
                // return end_turn without a single agent event. Do not claim success.
                DriverEvent::TurnFailed { reason: "OpenCode 未返回任何内容或工具活动，无法确认任务完成。请在原生客户端检查运行状态和日志后重试。".into() }
            } else { event })
        }));
        Ok(stream)
    }

    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        #[cfg(test)]
        let timeout = self.stop_timeout;
        #[cfg(not(test))]
        let timeout = std::time::Duration::from_secs(10);
        let receipt = {
            let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
            self.shared.validate_handle(handle)?;
            let turn = self
                .shared
                .latest_turn
                .lock()
                .expect("ACP turn")
                .clone()
                .ok_or_else(|| {
                    DriverError::CapabilityUnavailable("ACP has no turn to stop".into())
                })?;
            if turn.session != handle.native_session_id {
                return Err(DriverError::SessionMissing(
                    "ACP stop does not target the active session".into(),
                ));
            }
            if self.shared.setup_in_progress.load(Ordering::Acquire)
                || !self
                    .shared
                    .sessions
                    .lock()
                    .expect("ACP sessions")
                    .get(&handle.native_session_id)
                    .is_some_and(|resident| {
                        !resident.is_closing()
                            && Arc::ptr_eq(&resident.epoch, &turn.epoch)
                            && resident
                                .turn
                                .lock()
                                .expect("ACP session turn")
                                .as_ref()
                                .is_some_and(|current| Arc::ptr_eq(current, &turn))
                    })
                || self
                    .shared
                    .closed_sessions
                    .lock()
                    .expect("closed sessions")
                    .contains(&handle.native_session_id)
            {
                return Err(DriverError::SessionMissing(
                    "ACP stop no longer owns the original session".into(),
                ));
            }
            self.shared
                .interrupt_requested
                .store(true, Ordering::Release);
            self.shared.cancel_interactions(&handle.native_session_id);
            if self.executor.provider_id.as_str() == "codebuddy" {
                self.shared
                    .reconnect_required
                    .store(true, Ordering::Release);
            }
            turn.stop(timeout)
        };
        turn::wait(receipt, timeout + std::time::Duration::from_secs(1)).await
    }

    async fn respond_to_question(
        &self,
        handle: &SessionHandle,
        request_id: &str,
        answer: agent_domain::QuestionAnswer,
    ) -> Result<(), DriverError> {
        let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
        self.shared.validate_handle(handle)?;
        self.shared
            .validate_interaction(Some(&handle.native_session_id))?;
        let mut pending = self.shared.questions.lock().expect("questions");
        let question = pending
            .get(request_id)
            .ok_or_else(|| DriverError::Protocol("Question expired".into()))?;
        if !question.session_id.is_empty() && question.session_id != handle.native_session_id {
            return Err(DriverError::Protocol("Question session mismatch".into()));
        }
        question
            .form
            .validate(&answer)
            .map_err(DriverError::Protocol)?;
        pending
            .remove(request_id)
            .expect("checked question")
            .reply
            .send(answer)
            .map_err(|_| DriverError::Disconnected("Question closed".into()))
    }

    async fn respond_to_interaction(
        &self,
        handle: &SessionHandle,
        native_request_id: &str,
        option_id: &str,
    ) -> Result<(), DriverError> {
        let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
        self.shared.validate_handle(handle)?;
        self.shared
            .validate_interaction(Some(&handle.native_session_id))?;
        let mut permissions = self
            .shared
            .pending_permissions
            .lock()
            .expect("pending permissions");
        let pending = permissions.get(native_request_id).ok_or_else(|| {
            DriverError::CapabilityUnavailable("this approval request is no longer pending".into())
        })?;
        if pending.session_id != handle.native_session_id {
            return Err(DriverError::SessionMissing(
                "ACP approval session mismatch".into(),
            ));
        }
        // Validate before removal: a wrong session/option must not consume the
        // legitimate pending request, including concurrent response attempts.
        let Some(option) = pending
            .options
            .iter()
            .find(|option| option.option_id.0.as_ref() == option_id)
        else {
            return Err(DriverError::CapabilityUnavailable(
                "unknown approval option id".into(),
            ));
        };
        let option = option.clone();
        let pending = permissions
            .remove(native_request_id)
            .expect("checked approval");
        drop(permissions);
        if matches!(
            option.kind,
            PermissionOptionKind::RejectOnce | PermissionOptionKind::RejectAlways
        ) {
            // Record before waking the native responder: Grok can immediately
            // finish session/prompt with Cancelled after this response.
            self.shared.approval_rejected.store(true, Ordering::Release);
            self.shared
                .activity
                .lock()
                .expect("activity")
                .reject(&pending.session_id, &pending.tool_call_id);
        }
        pending
            .responder
            .respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    option.option_id.clone(),
                )),
            ))
            .map_err(|_| DriverError::Disconnected("ACP agent connection is closed".into()))
    }

    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        let resident = {
            let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
            self.shared
                .sessions
                .lock()
                .expect("ACP sessions")
                .get(&handle.native_session_id)
                .filter(|r| r.handle.same_scope(handle))
                .cloned()?
        };
        Some(Box::pin(async move {
            loop {
                if resident.shutdown_confirmed() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }))
    }

    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        let permit = self.shared.setup.clone().lock_owned().await;
        {
            let _gate = self.shared.lifecycle.lock().expect("ACP lifecycle");
            self.shared.validate_close_handle(&handle)?;
        }
        #[cfg(test)]
        let timeout = self.stop_timeout;
        #[cfg(not(test))]
        let timeout = std::time::Duration::from_secs(10);
        let receipt =
            sessions::begin_close(self.shared.clone(), &handle.native_session_id, timeout).await?;
        drop(permit);
        turn::wait(receipt, std::time::Duration::from_secs(10)).await
    }
}

/// Current official billing schema, with legacy monthly credit fallback.
fn parse_grok_billing_windows(billing: &serde_json::Value) -> Option<Vec<QuotaWindow>> {
    let config = billing.get("config")?;
    let period = config.get("currentPeriod");
    let start = period
        .and_then(|p| p.get("start"))
        .or_else(|| config.get("billingPeriodStart"))?
        .as_str()?;
    let end = period
        .and_then(|p| p.get("end"))
        .or_else(|| config.get("billingPeriodEnd"))?
        .as_str()?;
    let start = parse_iso8601_unix(start)?;
    let end = parse_iso8601_unix(end)?;
    let window_minutes = u64::try_from(end.checked_sub(start)? / 60)
        .ok()
        .filter(|n| *n > 0)?;
    let valid = |value: f64| value.is_finite() && value >= 0.0;
    let used_percent = config
        .get("creditUsagePercent")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| valid(*value))
        .or_else(|| {
            let number = |name| {
                config
                    .get(name)
                    .and_then(|v| v.get("val").unwrap_or(v).as_f64())
            };
            let used = number("used").filter(|value| valid(*value))?;
            let limit = number("monthlyLimit").filter(|value| valid(*value) && *value > 0.0)?;
            Some(used / limit * 100.0).filter(|value| value.is_finite())
        })
        // A config without a readable usage figure (the current Grok free tier
        // omits every usage field) must stay unknown: inventing a full balance
        // masks a quota that the token window may have already exhausted.
        .map(|value| value.clamp(0.0, 100.0));
    Some(vec![QuotaWindow {
        scope: None,
        id: period
            .and_then(|p| p.get("type"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("billing")
            .into(),
        window_minutes,
        used_percent,
        resets_at_unix: Some(end),
    }])
}

fn parse_iso8601_unix(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp())
}

/// Maps the verified `kimi web` `/api/v1/oauth/usage` payload to quota
/// windows. Shapes (real fetch, 2026-09-04):
/// `data.summary = {window:{duration:1,unit:"week"}, used, limit, reset_at}` and
/// `data.limits[]` entries with `window:{duration:5,unit:"hour"}`. Only
/// day/week/hour units are understood; others are skipped, never guessed.
/// Kimi 2.x instead returns `data.quota.usages.{limit5h,limit7d}` with
/// `usedRatio` (0–1) and `resetAt`. Prefer this shape when present.
fn parse_kimi_web_windows(data: &serde_json::Value) -> Vec<QuotaWindow> {
    fn window_minutes(duration: u64, unit: &str) -> Option<u64> {
        match unit {
            "hour" => Some(duration.saturating_mul(60)),
            "day" => Some(duration.saturating_mul(24 * 60)),
            "week" => Some(duration.saturating_mul(7 * 24 * 60)),
            _ => None,
        }
    }
    fn parse_window_node(node: &serde_json::Value, fallback_id: &str) -> Option<QuotaWindow> {
        let window = node.get("window")?;
        let duration = window.get("duration").and_then(serde_json::Value::as_u64)?;
        let unit = window.get("unit").and_then(serde_json::Value::as_str)?;
        let minutes = window_minutes(duration, unit)?;
        let used = node.get("used").and_then(serde_json::Value::as_f64)?;
        // Limits are small counts (e.g. 100 turns); u32 range is plenty and
        // converts to f64 losslessly.
        let limit = node
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .and_then(|limit| u32::try_from(limit).ok())
            .map(f64::from)?;
        if limit <= 0.0 || minutes == 0 || !used.is_finite() || used < 0.0 {
            return None;
        }
        let used_percent = ((used / limit) * 100.0).clamp(0.0, 100.0);
        // reset_at is ISO-8601 (same clock format Grok's billing reports).
        let reset_at = node
            .get("reset_at")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_iso8601_unix);
        Some(QuotaWindow {
            scope: None,
            id: fallback_id.to_owned(),
            window_minutes: minutes,
            used_percent: Some((used_percent * 100.0).round() / 100.0),
            resets_at_unix: reset_at,
        })
    }
    let mut windows = Vec::new();
    if let Some(quota) = data.get("quota") {
        for (id, minutes) in [("limit5h", 300), ("limit7d", 10_080)] {
            let node = &quota["usages"][id];
            let Some(ratio) = node.get("usedRatio").and_then(serde_json::Value::as_f64) else {
                continue;
            };
            if !ratio.is_finite() || ratio < 0.0 {
                continue;
            }
            windows.push(QuotaWindow {
                scope: None,
                id: id.into(),
                window_minutes: minutes,
                used_percent: Some(ratio.clamp(0.0, 1.0) * 100.0),
                resets_at_unix: node
                    .get("resetAt")
                    .and_then(serde_json::Value::as_str)
                    .and_then(parse_iso8601_unix),
            });
        }
        // Do not fall back to potentially stale legacy fields in a malformed new response.
        return windows;
    }
    if let Some(summary) = data.get("summary")
        && let Some(window) = parse_window_node(summary, "weekly_summary")
    {
        windows.push(window);
    }
    if let Some(limits) = data.get("limits").and_then(serde_json::Value::as_array) {
        for (index, node) in limits.iter().enumerate() {
            if let Some(window) = parse_window_node(node, &format!("limit_{index}")) {
                windows.push(window);
            }
        }
    }
    windows.sort_by_key(|window| window.window_minutes);
    windows.dedup_by(|left, right| left.window_minutes == right.window_minutes);
    windows
}

fn parse_kimi_web_quota(payload: &serde_json::Value) -> Result<Vec<QuotaWindow>, DriverError> {
    if payload
        .get("code")
        .is_some_and(|code| code.as_i64() != Some(0))
    {
        return Err(DriverError::Protocol(
            "Kimi usage service returned an error".into(),
        ));
    }
    let data = payload
        .get("data")
        .filter(|data| data.is_object())
        .ok_or_else(|| {
            DriverError::Protocol("kimi usage payload is missing its data object".into())
        })?;
    if data
        .get("kind")
        .is_some_and(|kind| kind.as_str() != Some("ok"))
    {
        return Err(
            match data.get("status").and_then(serde_json::Value::as_u64) {
                Some(401) => DriverError::NotConfigured(
                    "Kimi quota requires login; please sign in again".into(),
                ),
                Some(status) => DriverError::Protocol(format!(
                    "Kimi upstream usage request failed (HTTP {status})"
                )),
                None => DriverError::Protocol("Kimi upstream usage request failed".into()),
            },
        );
    }
    let windows = parse_kimi_web_windows(data);
    if windows.is_empty() {
        return Err(DriverError::Protocol(
            "kimi usage payload contained no recognizable windows".into(),
        ));
    }
    Ok(windows)
}

impl AcpDriver {
    async fn read_kimi_web_quota_windows(&self) -> Result<Vec<QuotaWindow>, DriverError> {
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(45),
            self.shared.metadata.clone().lock_owned(),
        )
        .await
        .map_err(|_| {
            DriverError::CapabilityUnavailable("上一项账户额度查询仍在结束，请稍后重试".into())
        })?;
        let payload = read_kimi_web_usage(&self.shared.launch.executable, permit).await?;
        parse_kimi_web_quota(&payload)
    }

    /// Context-only quota answer for agents whose only quota surface is the
    /// ACP `session/update` `UsageUpdate` push (Kimi, Cursor, ...). Windows
    /// stay empty — the hub never invents subscription limits.
    fn context_only_quota(
        &self,
        native_session_id: Option<&str>,
    ) -> Result<QuotaSnapshot, DriverError> {
        let context = self.shared.quota.context(native_session_id);
        let Some(context) = context else {
            return Err(DriverError::CapabilityUnavailable(
                "no ACP usage observation has arrived for this session yet".into(),
            ));
        };
        let now = now_unix_ms();
        Ok(QuotaSnapshot {
            session_usage: None,
            windows: vec![],
            context: Some(context),
            collected_at_unix_ms: now,
            stale_after_unix_ms: now.saturating_add(ACP_CONTEXT_TTL_MS),
            plan_type: None,
            source: "native_push".into(),
            confidence: "provider_reported".into(),
            parser_version: "acp-usage-update-v1".into(),
        })
    }
}

#[async_trait]
impl QuotaProvider for AcpDriver {
    async fn read_session_usage(
        &self,
        id: &str,
    ) -> Option<driver_api::usage::SessionUsageSnapshot> {
        let target = auxiliary::Snapshot::capture(&self.shared, id);
        if self.executor.provider_id.as_str() == "kiro" {
            self.refresh_kiro_context(id).await;
        }
        let directory = target.directory.clone();
        if let Some(directory) = directory
            && !(self.executor.provider_id.as_str() == "grok"
                && self.shared.turn_active.load(Ordering::Acquire))
        {
            let provider = self.executor.provider_id.to_string();
            let session = id.to_owned();
            let before = self.shared.quota.usage.get(Some(id));
            if let Ok(Some(usage)) = tokio::task::spawn_blocking(move || {
                let old = session_usage::read_native(&provider, &directory, &session);
                if provider == "codebuddy" {
                    return old;
                }
                if let Some(mut next) = request_usage::read_native(&provider, &directory, &session)
                {
                    if let Some(old) = old {
                        next.context_used_tokens = old.context_used_tokens;
                        next.context_window_tokens = old.context_window_tokens;
                    }
                    Some(next)
                } else {
                    old
                }
            })
            .await
            {
                let mut usage = usage;
                target.apply(&self.shared, || {
                    // A live push or newer read wins over this older file snapshot.
                    if self.shared.quota.usage.get(Some(id)) != before {
                        return;
                    }
                    if self.executor.provider_id.as_str() == "grok" {
                        let live = self
                            .shared
                            .quota
                            .usage
                            .get(Some(id))
                            .and_then(|s| s.request_usage);
                        if let Some(report) = usage.request_usage.take() {
                            usage.request_usage = Some(
                                self.shared
                                    .quota
                                    .grok_usage
                                    .lock()
                                    .expect("grok usage")
                                    .reconcile(id, report, live.as_ref()),
                            );
                        }
                    }
                    self.shared.quota.usage.merge(id, usage);
                })?;
            }
        }
        let mut usage = target.apply(&self.shared, || {
            self.shared.quota.usage.get(Some(id)).unwrap_or_default()
        })?;
        if usage.request_usage.is_none() {
            usage.request_usage = Some(driver_api::usage::RequestUsageReport::unavailable(
                match self.executor.provider_id.as_str() {
                    "kiro" => "原生仅提供 Credits 摘要，未确认逐请求 token",
                    "cursor" => "原生 ACP 未提供逐请求用量",
                    _ => "尚未取得当前原生会话的逐请求用量",
                },
            ));
        }
        Some(usage)
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
        if !matches!(
            self.executor.provider_id.as_str(),
            "kimi" | "grok" | "cursor" | "kiro"
        ) {
            return self.context_only_quota(native_session_id);
        }
        // Single-flight refresh across panel, task switches and event refreshes.
        let _refresh = self.shared.quota.refresh.lock().await;
        if !force
            && self.shared.quota.account_is_fresh()
            && let Some(snapshot) = self.shared.quota.account_snapshot(native_session_id)
        {
            return Ok(snapshot);
        }
        if !force
            && self.shared.quota.account_is_fresh()
            && let Some(error) = self.shared.quota.last_error.lock().unwrap().clone()
        {
            if driver_api::failure::invalidates_account_quota(&error) {
                return Err(error);
            }
            return self
                .context_only_quota(native_session_id)
                .map_err(|_| error);
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(27), async {
            if self.executor.provider_id.as_str() == "kiro" {
                let payload = self
                    .isolated_query("账户额度", None, |reply| Command::ReadKiroUsage {
                        reply,
                    })
                    .await?;
                return kiro_quota::parse(&payload);
            }
            if self.executor.provider_id.as_str() == "cursor" {
                return cursor_quota::read().await;
            }
            let (windows, plan_type, parser_version) =
                if self.executor.provider_id.as_str() == "kimi" {
                    (
                        self.read_kimi_web_quota_windows().await?,
                        None,
                        KIMI_OAUTH_USAGE_PARSER_VERSION,
                    )
                } else {
                    let read_once = || {
                        self.isolated_query("账户额度", None, |reply| {
                            Command::ReadGrokBilling { reply }
                        })
                    };
                    let billing = if let Ok(value) = read_once().await {
                        value
                    } else {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        read_once().await?
                    };
                    let windows = parse_grok_billing_windows(&billing).ok_or_else(|| {
                        DriverError::Protocol("Grok billing windows unavailable".into())
                    })?;
                    let plan = billing
                        .get("subscriptionTier")
                        .or_else(|| billing.get("subscription_tier"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    (windows, plan, GROK_BILLING_PARSER_VERSION)
                };
            let now = now_unix_ms();
            Ok::<_, DriverError>(QuotaSnapshot {
                session_usage: None,
                windows,
                context: None,
                collected_at_unix_ms: now,
                stale_after_unix_ms: now.saturating_add(GROK_QUOTA_TTL_MS_I64),
                plan_type,
                source: "native_pull".into(),
                confidence: "provider_reported".into(),
                parser_version: parser_version.into(),
            })
        })
        .await
        .unwrap_or_else(|_| Err(DriverError::Disconnected("Quota refresh timed out".into())));
        match result {
            Ok(snapshot) => {
                *self.shared.quota.last_error.lock().unwrap() = None;
                self.shared.quota.record_account(snapshot);
                Ok(self
                    .shared
                    .quota
                    .account_snapshot(native_session_id)
                    .unwrap())
            }
            Err(error) => {
                *self.shared.quota.last_error.lock().unwrap() = Some(error.clone());
                // Back off retries, but never renew the observation's stale deadline.
                self.shared.quota.account_fresh_until_unix_ms.store(
                    now_unix_ms().saturating_add(GROK_QUOTA_TTL_MS_I64),
                    Ordering::Release,
                );
                if driver_api::failure::invalidates_account_quota(&error) {
                    *self.shared.quota.account_snapshot.lock().unwrap() = None;
                    return Err(error);
                }
                if force {
                    return Err(error);
                }
                self.shared
                    .quota
                    .account_snapshot(native_session_id)
                    .or_else(|| self.context_only_quota(native_session_id).ok())
                    .ok_or(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn grok_approval_refusal_is_a_normal_turn_end() {
        for stop in [super::StopReason::Cancelled, super::StopReason::Refusal] {
            assert!(
                matches!(super::prompt_terminal(Ok(stop), true, true, false),
                super::DriverEvent::TurnCompleted { summary } if summary == "已拒绝工具操作，本轮已结束")
            );
        }
    }

    #[test]
    fn approval_refusal_does_not_hide_other_terminal_causes() {
        use super::*;
        for (grok, rejected, interrupted) in [
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            assert!(matches!(
                prompt_terminal(Ok(StopReason::Cancelled), grok, rejected, interrupted),
                DriverEvent::TurnInterrupted { .. }
            ));
        }
        assert!(matches!(
            prompt_terminal(
                Err(DriverError::Disconnected("fixture".into())),
                true,
                true,
                false
            ),
            DriverEvent::TurnFailed { .. }
        ));
        assert!(matches!(
            prompt_terminal(
                Err(DriverError::QuotaExhausted {
                    resets_at_unix: Some(123)
                }),
                true,
                true,
                false
            ),
            DriverEvent::TurnQuotaExhausted {
                resets_at_unix: Some(123)
            }
        ));
        assert!(matches!(
            prompt_terminal(Ok(StopReason::EndTurn), true, false, false),
            DriverEvent::TurnCompleted { .. }
        ));
    }
    use super::*;

    #[test]
    fn acp_failure_categories_survive_prompt_terminal() {
        for (message, expected) in [
            (
                "Failed to authenticate secret-token",
                "authentication_failed",
            ),
            (
                "NOT NULL constraint failed: session_message.seq secret-path",
                "database_incompatible",
            ),
            ("request timed out secret-host", "agent_timeout"),
            (
                "Internal error: Model exo-free has been deprecated.",
                "model_unavailable",
            ),
            ("unknown secret-data", "driver_protocol_error"),
        ] {
            let error = AcpError::new(-32_000, message);
            let mapped = map_session_error(&error, "ACP prompt rejected", None);
            let terminal = prompt_terminal(Err(mapped), false, false, false);
            let DriverEvent::TurnFailed { reason } = terminal else {
                panic!("expected a failed turn");
            };
            assert_eq!(
                driver_api::failure::split_turn_failure(&reason).0,
                Some(expected)
            );
        }
    }

    #[test]
    fn explicit_acp_usage_exhaustion_is_structured() {
        let error = AcpError::new(-32_000, "Usage limit reached for this account");
        assert!(matches!(
            map_session_error(&error, "private vendor text", None),
            DriverError::QuotaExhausted {
                resets_at_unix: None
            }
        ));
        let throttled = AcpError::new(-32_000, "HTTP 429 rate limit; retry shortly");
        assert!(matches!(
            map_session_error(&throttled, "stable context", None),
            DriverError::Protocol(message) if message == "stable context"
        ));
    }

    #[test]
    fn unexplained_failures_classify_the_captured_stderr_tail() {
        let error = AcpError::new(-32_000, "session unavailable");
        // Grok kills its quota worker before the JSON-RPC layer relays a reason;
        // the only usable signal is this stderr line.
        let stderr = StderrTail::default();
        let request = stderr.begin_request();
        stderr.record(
            "ERROR responses API error status=429 error_message=subscription:free-usage-exhausted: You've used all the included free usage for model grok-4.6 for now.",
        );
        assert!(matches!(
            map_session_error(&error, "ACP prompt rejected", Some(&request)),
            DriverError::QuotaExhausted {
                resets_at_unix: None
            }
        ));
        // A backend outage recorded on stderr becomes a connection failure.
        let stderr = StderrTail::default();
        let request = stderr.begin_request();
        stderr.record(
            "ERROR worker quit with fatal: Transport channel closed, when UnexpectedServerResponse(\"HTTP 503 Service Unavailable: \")",
        );
        assert!(matches!(
            map_session_error(&error, "ACP prompt rejected", Some(&request)),
            DriverError::Protocol(kind) if kind == "agent_connection_failed"
        ));
        // Without a matching stderr the generic context survives verbatim.
        let stderr = StderrTail::default();
        let request = stderr.begin_request();
        stderr.record("progress 40%");
        assert!(matches!(
            map_session_error(&error, "ACP prompt rejected", Some(&request)),
            DriverError::Protocol(message) if message == "ACP prompt rejected"
        ));
    }

    #[test]
    fn stderr_classification_excludes_previous_requests_without_clearing_in_flight_logs() {
        let stderr = StderrTail::default();
        let earlier = stderr.begin_request();
        stderr.record("ERROR subscription:free-usage-exhausted");
        let current = stderr.begin_request();
        let error = AcpError::new(-32_000, "session unavailable");
        assert!(matches!(
            map_session_error(&error, "ACP prompt rejected", Some(&current)),
            DriverError::Protocol(message) if message == "ACP prompt rejected"
        ));
        // Beginning a new request must not erase an older request's evidence.
        assert!(matches!(
            map_session_error(&error, "ACP prompt rejected", Some(&earlier)),
            DriverError::QuotaExhausted { .. }
        ));
        stderr.record("ERROR HTTP 503 Service Unavailable");
        assert!(matches!(
            map_session_error(&error, "ACP prompt rejected", Some(&current)),
            DriverError::Protocol(kind) if kind == "agent_connection_failed"
        ));
        let next = stderr.begin_request();
        assert!(matches!(
            map_session_error(&error, "ACP prompt rejected", Some(&next)),
            DriverError::Protocol(message) if message == "ACP prompt rejected"
        ));
    }

    #[test]
    fn stderr_tail_keeps_only_recent_error_lines_within_bounds() {
        let stderr = StderrTail::default();
        stderr.record("progress line without error keyword is ignored");
        for index in 0..STDERR_TAIL_CAPACITY + 3 {
            stderr.record(&format!("ERROR failure {index}"));
        }
        stderr.record(&format!("ERROR {}", "x".repeat(STDERR_LINE_MAX_CHARS + 50)));
        let tail = stderr.0.lock().expect("stderr tail");
        assert_eq!(tail.lines.len(), STDERR_TAIL_CAPACITY);
        assert!(tail.lines.iter().all(|(_, line)| line.contains("ERROR")));
        assert!(
            tail.lines
                .front()
                .is_some_and(|(_, line)| line.contains("failure 4"))
        );
        assert!(
            tail.lines
                .back()
                .is_some_and(|(_, line)| line.chars().count() <= STDERR_LINE_MAX_CHARS)
        );
        drop(tail);
        stderr.clear();
        assert!(stderr.0.lock().expect("stderr tail").lines.is_empty());
    }
    use agent_client_protocol::Agent;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    #[tokio::test(flavor = "current_thread")]
    async fn session_and_prompt_requests_do_not_reuse_stderr_after_success() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (client_write, agent_read) = tokio::io::duplex(8 * 1024);
                let (agent_write, client_read) = tokio::io::duplex(8 * 1024);
                let quota = Arc::new(AcpQuotaState::default());
                let session_stderr = quota.stderr_tail.clone();
                let prompt_stderr = quota.stderr_tail.clone();
                let sessions = std::sync::atomic::AtomicUsize::new(0);
                let prompts = std::sync::atomic::AtomicUsize::new(0);
                let agent = Agent.builder()
                    .on_receive_request(
                        async move |_request: NewSessionRequest,
                                    responder: Responder<agent_client_protocol::schema::v1::NewSessionResponse>,
                                    _connection| {
                            if sessions.fetch_add(1, Ordering::SeqCst) == 0 {
                                session_stderr.record("ERROR subscription:free-usage-exhausted");
                                responder.respond(agent_client_protocol::schema::v1::NewSessionResponse::new(SessionId::new("stderr-test")))
                            } else {
                                responder.respond_with_error(AcpError::new(-32_000, "session unavailable"))
                            }
                        },
                        on_receive_request!(),
                    )
                    .on_receive_request(
                        async move |_request: PromptRequest, responder: Responder<PromptResponse>, _connection| {
                            if prompts.fetch_add(1, Ordering::SeqCst) == 0 {
                                prompt_stderr.record("ERROR subscription:free-usage-exhausted");
                                responder.respond(PromptResponse::new(StopReason::EndTurn))
                            } else {
                                responder.respond_with_error(AcpError::new(-32_000, "session unavailable"))
                            }
                        },
                        on_receive_request!(),
                    );
                let agent_task = tokio::task::spawn_local(async move {
                    let _ = agent.connect_to(
                        ByteStreams::new(agent_write.compat_write(), agent_read.compat()),
                    ).await;
                });
                Client.builder().connect_with(
                    ByteStreams::new(client_write.compat_write(), client_read.compat()),
                    async move |connection| {
                        let params = serde_json::json!({"cwd":"/tmp", "mcpServers":[]});
                        models::session_request(&connection, "session/new", params.clone(), Some(&quota.stderr_tail)).await.unwrap();
                        assert!(matches!(
                            models::session_request(&connection, "session/new", params, Some(&quota.stderr_tail)).await,
                            Err(DriverError::Protocol(message)) if message == "Agent 无法创建或恢复会话"
                        ));
                        let states = Arc::new(std::sync::Mutex::new(session_state::SessionStates::default()));
                        for turn in 0..2 {
                            let (reply, response) = oneshot::channel();
                            handle_command(connection.clone(), Command::Prompt {
                            turn: None,
                                session_id: SessionId::new("stderr-test"),
                                blocks: vec![ContentBlock::Text(TextContent::new("probe"))],
                                image_compatibility: false,
                                reply,
                            }, false, serde_json::json!({}), Arc::clone(&states), Arc::clone(&quota)).await;
                            let result = response.await.unwrap();
                            if turn == 0 {
                                assert_eq!(result, Ok(StopReason::EndTurn));
                            } else {
                                assert!(matches!(result, Err(DriverError::Protocol(message)) if message == "agent rejected the ACP session/prompt request"));
                            }
                        }
                        Ok(())
                    },
                ).await.unwrap();
                agent_task.abort();
                let _ = agent_task.await;
            })
            .await;
    }

    #[test]
    fn grok_usage_handles_current_legacy_and_missing_percent() {
        let mut payload = serde_json::json!({"config": {
            "currentPeriod": {"type":"weekly", "start":"2026-09-01T00:00:00Z", "end":"2026-09-08T00:00:00Z"},
            "creditUsagePercent": 12.5
        }});
        let windows = parse_grok_billing_windows(&payload).unwrap();
        assert_eq!(windows[0].used_percent, Some(12.5));
        assert_eq!(windows[0].window_minutes, 10080);
        // Without any readable usage figure the window stays unknown: the free
        // tier omits every usage field while its token quota can be exhausted.
        payload["config"]
            .as_object_mut()
            .unwrap()
            .remove("creditUsagePercent");
        assert_eq!(
            parse_grok_billing_windows(&payload).unwrap()[0].used_percent,
            None
        );
        payload["config"]["creditUsagePercent"] = serde_json::json!(0);
        assert_eq!(
            parse_grok_billing_windows(&payload).unwrap()[0].used_percent,
            Some(0.0)
        );
        payload["config"]["creditUsagePercent"] = serde_json::Value::Null;
        assert_eq!(
            parse_grok_billing_windows(&payload).unwrap()[0].used_percent,
            None
        );
        payload["config"]["creditUsagePercent"] = serde_json::json!("invalid");
        assert_eq!(
            parse_grok_billing_windows(&payload).unwrap()[0].used_percent,
            None
        );
        assert!(parse_grok_billing_windows(&serde_json::json!({})).is_none());
        let legacy = serde_json::json!({"config":{
            "billingPeriodStart":"2026-09-01T00:00:00Z", "billingPeriodEnd":"2026-09-08T00:00:00Z",
            "monthlyLimit":{"val":200}, "used":{"val":50}
        }});
        assert_eq!(
            parse_grok_billing_windows(&legacy).unwrap()[0].used_percent,
            Some(25.0)
        );
        payload["config"]["currentPeriod"]["end"] = serde_json::json!("2026-02-30T00:00:00Z");
        assert!(parse_grok_billing_windows(&payload).is_none());
    }

    #[tokio::test]
    async fn cached_account_context_is_scoped_and_expiry_is_not_renewed() {
        let state = AcpQuotaState::default();
        state.context_by_session.lock().unwrap().insert(
            "a".into(),
            (
                now_unix_ms(),
                ContextUsageSnapshot {
                    used_tokens: 42,
                    window_tokens: 100,
                    cache_hit_rate_percent: None,
                },
            ),
        );
        state.record_account(QuotaSnapshot {
            session_usage: None,
            windows: vec![],
            context: None,
            collected_at_unix_ms: 1,
            stale_after_unix_ms: 2,
            plan_type: None,
            source: "native_pull".into(),
            confidence: "provider_reported".into(),
            parser_version: "test".into(),
        });
        assert!(state.account_snapshot(Some("b")).unwrap().context.is_none());
        assert_eq!(
            state
                .account_snapshot(Some("a"))
                .unwrap()
                .context
                .unwrap()
                .used_tokens,
            42
        );
        assert_eq!(state.account_snapshot(None).unwrap().stale_after_unix_ms, 2);
    }

    async fn mock_usage_response(response: Vec<u8>) -> Result<serde_json::Value, DriverError> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(&response).await;
        });
        let result = fetch_kimi_usage(port, "test-token").await;
        server.await.unwrap();
        result
    }

    #[tokio::test]
    async fn kimi_http_accepts_chunked_and_rejects_status_redirect_and_large_body() {
        let response =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n{\"a\":1}\r\n0\r\n\r\n";
        assert_eq!(
            mock_usage_response(response.to_vec()).await.unwrap()["a"],
            1
        );
        for status in ["401 Unauthorized", "302 Found"] {
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: 2\r\nLocation: http://example.com\r\n\r\n{{}}"
            );
            assert!(mock_usage_response(response.into_bytes()).await.is_err());
        }
        let mut response = b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n".to_vec();
        response.extend(vec![b' '; 1_048_577]);
        assert!(mock_usage_response(response).await.is_err());
    }

    #[tokio::test]
    async fn failed_kimi_reads_back_off_without_starting_acp() {
        let driver = AcpDriver::new(
            "kimi",
            &agent_domain::InstallationId::new().to_string(),
            AcpLaunch {
                executable: PathBuf::from("/missing-kimi-test"),
                args: vec!["acp".into()],
            },
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        assert!(driver.read_quota(None).await.is_err());
        let until = driver
            .shared
            .quota
            .account_fresh_until_unix_ms
            .load(Ordering::Acquire);
        assert!(driver.read_quota(None).await.is_err());
        assert_eq!(
            until,
            driver
                .shared
                .quota
                .account_fresh_until_unix_ms
                .load(Ordering::Acquire)
        );
        assert!(driver.shared.connection.lock().await.is_none());
    }

    #[tokio::test]
    #[ignore = "requires local logged-in Kimi; read-only, no model turn"]
    async fn real_kimi_subscription_read_only() {
        let executable = PathBuf::from(std::env::var("HOME").unwrap()).join(".kimi-code/bin/kimi");
        let driver = AcpDriver::new(
            "kimi",
            &agent_domain::InstallationId::new().to_string(),
            AcpLaunch {
                executable,
                args: vec!["acp".into()],
            },
            None,
            SessionResumeCapability::Unknown,
        )
        .unwrap();
        let snapshot = driver
            .read_quota(None)
            .await
            .expect("Kimi read-only subscription read");
        assert!(
            snapshot
                .windows
                .iter()
                .any(|window| window.window_minutes == 300)
        );
        assert!(
            snapshot
                .windows
                .iter()
                .all(|window| window.used_percent.is_some())
        );
        assert!(snapshot.context.is_none());
        let cached = driver.read_quota(None).await.unwrap();
        assert_eq!(snapshot.collected_at_unix_ms, cached.collected_at_unix_ms);
        assert!(driver.shared.connection.lock().await.is_none());
    }

    #[allow(dead_code)]
    fn launches_are_structured() {
        let launch = AcpLaunch {
            executable: PathBuf::from("/usr/local/bin/kimi"),
            args: vec!["acp".into()],
        };
        assert!(launch.args.iter().all(|arg| !arg.starts_with('-')));
    }

    #[test]
    fn kimi_web_usage_payload_maps_both_windows() {
        // Real fetch shape (2026-09-04).
        let payload: serde_json::Value = serde_json::json!({
            "code": 0,
            "data": {
                "kind": "ok",
                "summary": {
                    "window": {"duration": 1, "unit": "week"},
                    "used": 13, "limit": 100,
                    "reset_at": "2026-09-06T14:27:41.373665Z"
                },
                "limits": [
                    {"window": {"duration": 5, "unit": "hour"},
                     "used": 0, "limit": 100,
                     "reset_at": "2026-09-05T07:27:41.373665Z"}
                ],
                "extra_usage": null
            }
        });
        let windows = parse_kimi_web_quota(&payload).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].window_minutes, 300);
        assert_eq!(windows[0].used_percent, Some(0.0));
        assert_eq!(
            windows[0].resets_at_unix,
            parse_iso8601_unix("2026-09-05T07:27:41.373665Z")
        );
        assert_eq!(windows[1].window_minutes, 10_080);
        assert_eq!(windows[1].used_percent, Some(13.0));
        assert_eq!(
            windows[1].resets_at_unix,
            parse_iso8601_unix("2026-09-06T14:27:41.373665Z")
        );
    }

    #[test]
    fn kimi_v2_usage_maps_real_response_without_rounding_to_exhaustion() {
        let mut payload = serde_json::json!({"code":0,"data":{"kind":"ok","quota":{
            "usages":{
                "limit5h":{"usedRatio":0,"resetAt":"2026-09-20T07:27:40Z"},
                "limit7d":{"usedRatio":0.342_791,"resetAt":"2026-09-20T14:27:40Z"}
            },"extraUsage":null
        }}});
        let windows = parse_kimi_web_quota(&payload).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].window_minutes, 300);
        assert_eq!(windows[0].used_percent, Some(0.0));
        assert_eq!(
            windows[0].resets_at_unix,
            parse_iso8601_unix("2026-09-20T07:27:40Z")
        );
        assert_eq!(windows[1].window_minutes, 10_080);
        assert!((windows[1].used_percent.unwrap() - 34.2791).abs() < 0.00001);
        assert_eq!(
            windows[1].resets_at_unix,
            parse_iso8601_unix("2026-09-20T14:27:40Z")
        );
        payload["data"]["quota"]["usages"]["limit5h"]["usedRatio"] = serde_json::json!(0.999_999);
        assert!(
            parse_kimi_web_quota(&payload).unwrap()[0]
                .used_percent
                .unwrap()
                < 100.0
        );
        payload["data"]["quota"]["usages"]["limit5h"]["usedRatio"] = serde_json::json!(1.1);
        assert_eq!(
            parse_kimi_web_quota(&payload).unwrap()[0].used_percent,
            Some(100.0)
        );
    }

    #[test]
    fn kimi_v2_missing_invalid_and_unknown_windows_are_not_invented() {
        let mut payload = serde_json::json!({"data":{"kind":"ok","quota":{"usages":{
            "limit5h":{"usedRatio":-1},
            "limit7d":{"usedRatio":0.5,"resetAt":"invalid"},
            "unknown":{"usedRatio":0}
        }}}});
        let windows = parse_kimi_web_quota(&payload).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].window_minutes, 10_080);
        assert_eq!(windows[0].resets_at_unix, None);
        payload["data"]["quota"]["usages"]["limit7d"]["usedRatio"] = serde_json::json!("0.5");
        // Legacy data must not hide an invalid v2 response.
        payload["data"]["summary"] =
            serde_json::json!({"window":{"duration":1,"unit":"week"},"used":0,"limit":100});
        assert!(parse_kimi_web_quota(&payload).is_err());
        for data in [
            serde_json::json!({"kind":"ok","quota":null}),
            serde_json::json!({"kind":"ok","quota":{"usages":{}}}),
            serde_json::Value::Null,
        ] {
            assert!(parse_kimi_web_quota(&serde_json::json!({"data":data})).is_err());
        }
    }

    #[test]
    fn kimi_business_errors_are_not_reported_as_empty_quota() {
        let payload = |status| serde_json::json!({"code":0,"data":{"kind":"error","status":status,"message":"private upstream detail"}});
        assert!(matches!(
            parse_kimi_web_quota(&payload(401)),
            Err(DriverError::NotConfigured(_))
        ));
        for status in [429, 500] {
            let error = parse_kimi_web_quota(&payload(status)).unwrap_err();
            assert!(matches!(error, DriverError::Protocol(_)));
            assert!(error.to_string().contains(&status.to_string()));
            assert!(!error.to_string().contains("private upstream detail"));
        }
        assert!(parse_kimi_web_quota(&serde_json::json!({"code":1,"data":{"kind":"ok"}})).is_err());
        assert!(parse_kimi_web_quota(&serde_json::json!({"data":{"kind":"error"}})).is_err());
    }

    /// Metadata only: no prompt, model turn, project tools, or MCP servers.
    #[tokio::test]
    #[ignore = "creates native metadata sessions; requires a locally authenticated ACP binary"]
    async fn local_startup_breakdown() {
        let binary = std::env::var("DOGCODES_ACP_BINARY").expect("DOGCODES_ACP_BINARY");
        let launch = AcpLaunch {
            executable: binary.into(),
            args: vec!["--disable-auto-update".into(), "acp".into()],
        };
        let directory = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let (mut child, transport) = spawn_agent(&launch).unwrap();
        println!(
            "startup_breakdown stage=process_spawn duration_ms={:.2}",
            started.elapsed().as_secs_f64() * 1000.0
        );
        let result = Client
            .builder()
            .connect_with(transport, async |connection| {
                let started = std::time::Instant::now();
                initialize(&connection).await?;
                println!(
                    "startup_breakdown stage=initialize duration_ms={:.2}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
                for index in 1..=3 {
                    let started = std::time::Instant::now();
                    let state = models::session_request(
                        &connection,
                        "session/new",
                        serde_json::json!({"cwd":directory.path(),"mcpServers":[]}),
                        None,
                    )
                    .await
                    .unwrap();
                    println!(
                        "startup_breakdown stage=session_new_{index} duration_ms={:.2}",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    let session = SessionId::new(state["sessionId"].as_str().unwrap());
                    let started = std::time::Instant::now();
                    let state = models::select(&connection, &state, &session, None, None)
                        .await
                        .unwrap();
                    println!(
                        "startup_breakdown stage=model_selection_{index} duration_ms={:.2}",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                    let started = std::time::Instant::now();
                    permissions::select(&connection, &state, &session, None)
                        .await
                        .unwrap();
                    println!(
                        "startup_breakdown stage=permission_selection_{index} duration_ms={:.2}",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
                Ok(())
            })
            .await;
        if tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        result.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn prompt_streams_text_and_terminates_with_the_stop_reason() {
        // LocalSet carries the fake agent side; the driver runs as a normal
        // task on the current-thread runtime.
        tokio::task::LocalSet::new()
            .run_until(async {
                let (client_write, agent_read) = tokio::io::duplex(8 * 1024);
                let (agent_write, client_read) = tokio::io::duplex(8 * 1024);
                let agent_transport =
                    ByteStreams::new(agent_write.compat_write(), agent_read.compat());
                let session_id = SessionId::new("acp-test-session");
                let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let observed_loads = loads.clone();
                let created_sessions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let observed_sessions = created_sessions.clone();
                let new_session = session_id.clone();
                let prompt_session = session_id.clone();
                let agent = Agent
                    .builder()
                    .on_receive_request(
                        async |request: InitializeRequest, responder, _connection| {
                            responder.respond(
                                InitializeResponse::new(request.protocol_version)
                                    .agent_capabilities(
                                        AgentCapabilities::new().load_session(true),
                                    ),
                            )
                        },
                        on_receive_request!(),
                    )
                    .on_receive_request(
                        async move |request: NewSessionRequest,
                                    responder: Responder<
                            agent_client_protocol::schema::v1::NewSessionResponse,
                        >,
                                    _connection| {
                            let index = observed_sessions.fetch_add(1, Ordering::SeqCst);
                            assert!(request.mcp_servers.is_empty());
                            if index == 0 {
                                return responder.respond_with_error(AcpError::auth_required());
                            }
                            let id = if index == 1 {
                                assert!(request.cwd.is_dir());
                                SessionId::new("metadata-only-probe")
                            } else {
                                assert_eq!(request.cwd, PathBuf::from("/tmp/acp-driver-test"));
                                new_session.clone()
                            };
                            responder.respond(
                                agent_client_protocol::schema::v1::NewSessionResponse::new(id),
                            )
                        },
                        on_receive_request!(),
                    )
                    .on_receive_request(
                        async move |_request: LoadSessionRequest,
                                    responder: Responder<
                            agent_client_protocol::schema::v1::LoadSessionResponse,
                        >,
                                    _connection| {
                            observed_loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            responder.respond(
                                agent_client_protocol::schema::v1::LoadSessionResponse::new(),
                            )
                        },
                        on_receive_request!(),
                    )
                    .on_receive_request(
                        async move |request: PromptRequest,
                                    responder: Responder<PromptResponse>,
                                    connection| {
                            assert_eq!(request.session_id, prompt_session);
                            // The context block must precede the user prompt.
                            assert_eq!(request.prompt.len(), 2);
                            connection
                                .send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new("streamed answer")),
                                    )),
                                ))
                                .expect("notification send should not fail");
                            connection.send_notification(SessionNotification::new(
                                request.session_id.clone(),
                                SessionUpdate::UsageUpdate(
                                    agent_client_protocol::schema::v1::UsageUpdate::new(120, 1000),
                                ),
                            ))?;
                            responder.respond(PromptResponse::new(StopReason::EndTurn).usage(
                                agent_client_protocol::schema::v1::Usage::new(9000, 8000, 1000),
                            ))
                        },
                        on_receive_request!(),
                    );
                tokio::task::spawn_local(async move {
                    let _ = agent.connect_to(agent_transport).await;
                });

                let transport = ByteStreams::new(client_write.compat_write(), client_read.compat());
                let updates: Updates = Arc::new(std::sync::Mutex::new(None));
                let (commands_tx, commands_rx) = mpsc::unbounded_channel();
                let (_cancel_tx, cancel_rx) = mpsc::unbounded_channel();
                let tasks = ConnectionTasks {
                    owner: None,
                    background: lifecycle::Tasks::new(),
                    activity: Arc::new(std::sync::Mutex::new(activity::Activities::default())),
                    updates: Arc::clone(&updates),
                    pending: Arc::new(std::sync::Mutex::new(HashMap::new())),
                    questions: Arc::new(std::sync::Mutex::new(HashMap::new())),
                    quota: Arc::new(AcpQuotaState::default()),
                };
                let observed_quota = Arc::clone(&tasks.quota);
                tokio::spawn(async move {
                    run_connection(transport, commands_rx, cancel_rx, tasks).await;
                });

                let (reply, response) = oneshot::channel();
                commands_tx.send(Command::ProbeReady { reply }).unwrap();
                assert!(matches!(
                    response.await.unwrap(),
                    Err(DriverError::NotConfigured(_))
                ));
                let (reply, response) = oneshot::channel();
                commands_tx.send(Command::ProbeReady { reply }).unwrap();
                let report = response.await.unwrap().unwrap();
                assert_eq!(report.resume_capability, SessionResumeCapability::Supported);
                assert_eq!(created_sessions.load(Ordering::SeqCst), 2);
                // Timed-out callers must not leave metadata creation queued.
                let (reply, response) = oneshot::channel();
                drop(response);
                commands_tx.send(Command::ProbeReady { reply }).unwrap();

                let (reply_tx, reply_rx) = oneshot::channel();
                commands_tx
                    .send(Command::NewSession {
                        history_mcp: None,
                        grok: false,
                        permission_mode: None,
                        model: None,
                        thinking: None,
                        cwd: PathBuf::from("/tmp/acp-driver-test"),
                        additional_directories: vec![],
                        reply: reply_tx,
                    })
                    .expect("command channel should be open");
                let created = reply_rx
                    .await
                    .expect("reply task alive")
                    .expect("session ok");
                assert_eq!(created, session_id);
                assert_eq!(created_sessions.load(Ordering::SeqCst), 3);

                let (event_tx, mut event_rx) = mpsc::unbounded_channel();
                *updates.lock().expect("sink") = Some(event_tx);
                let (prompt_reply, prompt_reply_rx) = oneshot::channel();
                commands_tx
                    .send(Command::Prompt {
                        turn: None,
                        image_compatibility: false,
                        session_id: session_id.clone(),
                        blocks: vec![
                            ContentBlock::Text(TextContent::new("[dogcodes Handoff]")),
                            ContentBlock::Text(TextContent::new("do the thing")),
                        ],
                        reply: prompt_reply,
                    })
                    .expect("command channel should be open");

                // ACP orders session updates before the prompt response, so
                // the streamed text is already queued once the reply resolves.
                let stop = prompt_reply_rx
                    .await
                    .expect("prompt reply")
                    .expect("prompt ok");
                assert_eq!(stop, StopReason::EndTurn);
                let usage = observed_quota
                    .usage
                    .get(Some(session_id.0.as_ref()))
                    .unwrap();
                assert_eq!(usage.context_used_tokens, Some(120));
                assert_eq!(usage.context_window_tokens, Some(1000));
                assert_eq!(usage.total_tokens, Some(9000));
                assert!(observed_quota.usage.get(Some("other-session")).is_none());
                let mut streamed = String::new();
                while let Ok(event) = event_rx.try_recv() {
                    match event {
                        DriverEvent::TextDelta { text, .. } => streamed.push_str(&text),
                        DriverEvent::Presentation { data, .. } if data["type"] == "message" => {
                            streamed.push_str(data["textDelta"].as_str().unwrap_or(""));
                        }
                        _ => {}
                    }
                }
                assert_eq!(streamed, "streamed answer");
                // A second turn on the same connection uses the resident session.
                // Changed workspace or MCP configuration must still restore.
                for (cwd, directories, expected_loads) in [
                    ("/tmp/acp-driver-test", vec![], 0),
                    ("/tmp/changed", vec![], 1),
                    ("/tmp/changed", vec![], 1),
                    ("/tmp/acp-driver-test", vec![], 2),
                ] {
                    let (reply, response) = oneshot::channel();
                    commands_tx
                        .send(Command::LoadSession {
                            history_mcp: None,
                            grok: false,
                            permission_mode: None,
                            model: None,
                            thinking: None,
                            session_id: session_id.clone(),
                            cwd: PathBuf::from(cwd),
                            additional_directories: directories,
                            reply,
                        })
                        .unwrap();
                    assert_eq!(response.await.unwrap().unwrap(), session_id);
                    assert_eq!(
                        loads.load(std::sync::atomic::Ordering::SeqCst),
                        expected_loads
                    );
                }
            })
            .await;
    }
}

/// Offline-only contract test above stays default; this probe runs against the
/// real local installations and is executed explicitly with `--ignored`.
#[cfg(test)]
mod real_installations {
    use super::*;

    async fn probe(name: &str, executable: &str, args: &[&str]) {
        let launch = AcpLaunch {
            executable: PathBuf::from(executable),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        };
        match AcpDriver::probe_ready(&launch).await {
            Ok(report) => println!(
                "{name}: ACP v1 ready (resume={:?}, version={:?})",
                report.resume_capability, report.native_version
            ),
            Err(error) => panic!("{name}: probe failed: {error}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires the local Kimi Code installation; run with --ignored"]
    async fn probes_the_current_users_kimi_installation() {
        let home = std::env::var("HOME").expect("HOME");
        probe("kimi", &format!("{home}/.kimi-code/bin/kimi"), &["acp"]).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires the local Grok Build installation; run with --ignored"]
    async fn probes_the_current_users_grok_installation() {
        let home = std::env::var("HOME").expect("HOME");
        probe(
            "grok",
            &format!("{home}/.local/bin/grok"),
            &["--permission-mode", "default", "agent", "stdio"],
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires the official claude-agent-acp adapter; run with --ignored"]
    async fn probes_the_current_users_claude_acp_adapter() {
        let home = std::env::var("HOME").expect("HOME");
        probe(
            "claude-code (claude-agent-acp)",
            &format!("{home}/.nvm/versions/node/v22.14.0/bin/claude-agent-acp"),
            &[],
        )
        .await;
    }
}

/// Real-turn acceptance per native ACP agent; consumes real model quota and
/// requires the operator's explicit authorization. Fixed no-tool prompts.
#[cfg(test)]
mod real_turn_acceptance {
    use super::*;
    use driver_api::SessionRequest;

    const TURN_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(3);

    async fn run_acceptance(name: &'static str, launch: AcpLaunch) {
        let report = match AcpDriver::probe_ready(&launch).await {
            Ok(report) => report,
            // Honest unauthenticated state: skip until the operator logs in
            // inside the agent; the hub must not fake a turn.
            Err(DriverError::NotConfigured(_)) => {
                println!(
                    "{name}: not logged in; turn acceptance skipped until the agent's own login flow completes"
                );
                return;
            }
            Err(error) => panic!("{name}: readiness probe failed: {error}"),
        };
        let driver = AcpDriver::new(
            name,
            &agent_domain::InstallationId::new().to_string(),
            launch,
            report.native_version,
            report.resume_capability,
        )
        .expect("driver construction must succeed");
        let session = match driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
            })
            .await
        {
            Ok(session) => session,
            // Honest unauthenticated state: the acceptance cannot run until the
            // operator logs in inside the agent; the hub must not fake a turn.
            Err(DriverError::NotConfigured(_)) => {
                println!(
                    "{name}: not logged in; turn acceptance skipped until the agent's own login flow completes"
                );
                return;
            }
            Err(error) => panic!("{name}: session/new failed unexpectedly: {error}"),
        };

        let first: Vec<DriverEvent> = tokio::time::timeout(
            TURN_TIMEOUT,
            driver
                .prompt(
                    &session,
                    HubPromptRequest {
                        attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt: "Reply with exactly: OK".into(),
                        working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
                        context: vec![],
                    },
                )
                .await
                .expect("turn 1 must be accepted")
                .map(|event| event.unwrap())
                .collect::<Vec<_>>(),
        )
        .await
        .expect("turn 1 must finish in time");
        assert!(
            first.iter().any(|event| matches!(
                event,
                DriverEvent::TextDelta { text, .. } if !text.is_empty()
            )),
            "{name}: turn 1 must stream text"
        );
        assert!(
            first
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "{name}: turn 1 must complete"
        );
        println!("{name}: turn 1 (stream + complete): ok");

        let second: Vec<DriverEvent> = tokio::time::timeout(
            TURN_TIMEOUT,
            driver
                .prompt(
                    &session,
                    HubPromptRequest {
                        attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt: "Reply with exactly: DONE".into(),
                        working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
                        context: vec![],
                    },
                )
                .await
                .expect("turn 2 must be accepted")
                .map(|event| event.unwrap())
                .collect::<Vec<_>>(),
        )
        .await
        .expect("turn 2 must finish in time");
        assert!(
            second
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "{name}: turn 2 must complete on the same live session"
        );
        println!("{name}: turn 2 (live session reuse): ok");

        let stream = driver
            .prompt(
                &session,
                HubPromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "Count from 1 to 20 slowly, one number per line.".into(),
                    working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
                    context: vec![],
                },
            )
            .await
            .expect("turn 3 must be accepted");
        let collector = tokio::spawn(async move {
            tokio::time::timeout(
                TURN_TIMEOUT,
                stream.map(|event| event.unwrap()).collect::<Vec<_>>(),
            )
            .await
            .expect("turn 3 must finish in time")
        });
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        driver
            .interrupt(&session)
            .await
            .expect("interrupt must send");
        let third = collector.await.unwrap();
        assert!(
            third
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnInterrupted { .. })),
            "{name}: turn 3 must end interrupted: {third:?}"
        );
        println!("{name}: turn 3 (interrupt): ok");
    }

    fn home_executable(relative: &str, args: &[&str]) -> AcpLaunch {
        let home = std::env::var("HOME").expect("HOME");
        AcpLaunch {
            executable: PathBuf::from(format!("{home}/{relative}")),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_turn_acceptance_kimi() {
        run_acceptance("kimi", home_executable(".kimi-code/bin/kimi", &["acp"])).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_turn_acceptance_grok() {
        run_acceptance(
            "grok",
            home_executable(
                ".local/bin/grok",
                &["--permission-mode", "default", "agent", "stdio"],
            ),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_turn_acceptance_cursor() {
        run_acceptance(
            "cursor",
            home_executable(".local/bin/cursor-agent", &["acp"]),
        )
        .await;
    }
}

/// Cross-process resume acceptance: turn 1 in one driver instance, then a
/// fresh instance must re-attach via `session/load` and complete turn 2.
#[cfg(test)]
mod real_resume_acceptance {
    use super::*;
    use driver_api::SessionRequest;

    async fn run_resume(name: &'static str, launch: AcpLaunch) {
        let report = AcpDriver::probe_ready(&launch)
            .await
            .unwrap_or_else(|error| panic!("{name}: readiness probe failed: {error}"));
        let native_version = report.native_version.clone();
        let first = AcpDriver::new(
            name,
            &agent_domain::InstallationId::new().to_string(),
            launch.clone(),
            native_version.clone(),
            report.resume_capability,
        )
        .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let session = first
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: workspace.path().to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        let native_session_id = session.native_session_id.clone();
        let recall_token =
            format!("recall-{}", agent_domain::InstallationId::new()).to_ascii_lowercase();
        let events: Vec<DriverEvent> = tokio::time::timeout(
            std::time::Duration::from_mins(3),
            first
                .prompt(
                    &session,
                    HubPromptRequest {
                        attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt: format!("Remember this exact token for later: {recall_token}. Reply with exactly: remembered. Do not use tools."),
                        working_directory: workspace.path().to_string_lossy().into_owned(),
                        context: vec![],
                    },
                )
                .await
                .unwrap()
                .map(|event| event.unwrap())
                .collect::<Vec<_>>(),
        )
        .await
        .expect("turn 1 must finish");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
        first.shutdown(session).await.unwrap();

        // Fresh instance, as after an app restart: session/load must restore
        // the same native session with its context.
        let second = AcpDriver::new(
            name,
            &agent_domain::InstallationId::new().to_string(),
            launch,
            native_version.clone(),
            report.resume_capability,
        )
        .unwrap();
        let resumed = second
            .resume_session(
                &native_session_id,
                workspace.path().to_string_lossy().as_ref(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.native_session_id, native_session_id);
        let events: Vec<DriverEvent> = tokio::time::timeout(
            std::time::Duration::from_mins(3),
            second
                .prompt(
                    &resumed,
                    HubPromptRequest {
                        attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt:
                            "What exact token did I ask you to remember? Reply with exactly that token. Do not use tools."
                                .into(),
                        working_directory: workspace.path().to_string_lossy().into_owned(),
                        context: vec![],
                    },
                )
                .await
                .unwrap()
                .map(|event| event.unwrap())
                .collect::<Vec<_>>(),
        )
        .await
        .expect("turn 2 must finish");
        for event in &events {
            match event {
                DriverEvent::Presentation { data, .. } if data["type"] == "message" => eprintln!(
                    "{name}: presentation type={} text={}",
                    data["type"], data["textDelta"]
                ),
                DriverEvent::ProtocolWarning { message } => eprintln!("{name}: warning {message}"),
                DriverEvent::TurnFailed { reason } => eprintln!("{name}: failed {reason}"),
                _ => {}
            }
        }
        let text = events
            .iter()
            .filter_map(|event| match event {
                DriverEvent::TextDelta { text, .. } => Some(text.to_ascii_lowercase()),
                DriverEvent::Presentation { data, .. } if data["type"] == "message" => data["text"]
                    .as_str()
                    .or_else(|| data["textDelta"].as_str())
                    .map(str::to_ascii_lowercase),
                _ => None,
            })
            .collect::<String>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
        assert!(
            text.contains(&recall_token),
            "{name}: resume must restore context: {text}"
        );
        println!("{name}: real resume (reattach + recall): ok");
    }

    fn home_executable(relative: &str, args: &[&str]) -> AcpLaunch {
        let home = std::env::var("HOME").expect("HOME");
        AcpLaunch {
            executable: PathBuf::from(format!("{home}/{relative}")),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_resume_acceptance_kimi() {
        run_resume("kimi", home_executable(".kimi-code/bin/kimi", &["acp"])).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_resume_acceptance_grok() {
        run_resume(
            "grok",
            home_executable(
                ".local/bin/grok",
                &["--permission-mode", "default", "agent", "stdio"],
            ),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_resume_acceptance_codebuddy() {
        let executable = std::env::var_os("DOGCODES_ACP_BINARY")
            .map(PathBuf::from)
            .expect("Set DOGCODES_ACP_BINARY to the CodeBuddy CLI executable");
        run_resume(
            "codebuddy",
            AcpLaunch {
                executable,
                args: vec!["--acp".into()],
            },
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_resume_acceptance_cursor() {
        run_resume(
            "cursor",
            home_executable(".local/bin/cursor-agent", &["acp"]),
        )
        .await;
    }
}

/// Real Kimi usage probe: one minimal turn, then `read_quota` must answer
/// with a context-only snapshot if (and only if) Kimi pushed
/// `session/update` `UsageUpdate` during the turn.
#[cfg(test)]
mod real_kimi_quota_probe {
    use super::*;
    use driver_api::{PromptRequest as HubPromptRequest, SessionRequest};

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes one real model turn; run only with explicit operator authorization"]
    async fn kimi_usage_update_feeds_context_quota() {
        let home = std::env::var("HOME").expect("HOME");
        let launch = AcpLaunch {
            executable: PathBuf::from(format!("{home}/.kimi-code/bin/kimi")),
            args: ["acp"].iter().map(|arg| (*arg).to_owned()).collect(),
        };
        let report = AcpDriver::probe_ready(&launch)
            .await
            .expect("kimi probe must pass");
        let driver = AcpDriver::new(
            "kimi",
            &agent_domain::InstallationId::new().to_string(),
            launch,
            report.native_version,
            report.resume_capability,
        )
        .unwrap();
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        // Before any turn the session context is unknown, but the
        // subscription windows (via the local management server) may already
        // answer — both outcomes are honest.
        let events: Vec<DriverEvent> = tokio::time::timeout(
            std::time::Duration::from_mins(3),
            driver
                .prompt(
                    &session,
                    HubPromptRequest {
                        attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt: "Reply with exactly: OK".into(),
                        working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
                        context: vec![],
                    },
                )
                .await
                .unwrap()
                .map(|event| event.unwrap())
                .collect::<Vec<_>>(),
        )
        .await
        .expect("turn must finish");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
        );
        // Subscription pull is independent. Context is optional: the driver
        // does not inject a /usage prompt merely to obtain an observation.
        let snapshot = driver
            .read_quota(Some(&session.native_session_id))
            .await
            .expect("kimi pull must return a snapshot");
        if let Some(context) = snapshot.context {
            assert!(context.window_tokens > 0);
        }
        assert!(
            snapshot
                .windows
                .iter()
                .any(|window| window.window_minutes == 10_080),
            "the weekly subscription window must be present"
        );
        driver.shutdown(session).await.unwrap();
    }
}

/// Real Grok billing probe: verifies the `_x.ai/billing` extension responds
/// and prints the payload shape (no model call, no credentials touched).
#[cfg(test)]
mod real_quota_probe {
    use super::*;
    use driver_api::SessionRequest;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires the local logged-in Grok installation; run with --ignored"]
    async fn probes_the_current_users_grok_billing_extension() {
        let home = std::env::var("HOME").expect("HOME");
        let launch = AcpLaunch {
            executable: PathBuf::from(format!("{home}/.local/bin/grok")),
            args: ["--permission-mode", "default", "agent", "stdio"]
                .iter()
                .map(|arg| (*arg).to_owned())
                .collect(),
        };
        let report = AcpDriver::probe_ready(&launch)
            .await
            .expect("probe must pass");
        let driver = AcpDriver::new(
            "grok",
            &agent_domain::InstallationId::new().to_string(),
            launch,
            report.native_version,
            report.resume_capability,
        )
        .unwrap();
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
            })
            .await
            .expect("session must be created");
        let snapshot = driver
            .read_quota(Some(&session.native_session_id))
            .await
            .expect("Grok billing extension must return a quota snapshot");
        assert!(!snapshot.windows.is_empty());
        assert!(
            snapshot
                .windows
                .iter()
                .all(|window| window.resets_at_unix.is_some())
        );
        let cached = driver
            .read_quota(Some(&session.native_session_id))
            .await
            .unwrap();
        assert_eq!(snapshot.collected_at_unix_ms, cached.collected_at_unix_ms);
        driver.shutdown(session).await.unwrap();
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    #[test]
    fn image_negotiation_and_verified_grok_exception_are_separate() {
        let no_image = serde_json::json!({"agentCapabilities":{"promptCapabilities":{"embeddedContext":true}}});
        assert!(!supports_prompt_images(&no_image, false));
        assert!(supports_prompt_images(
            &no_image,
            grok_image_compatibility("grok", Some("1.0.30"))
        ));
        assert!(!grok_image_compatibility("kimi", Some("1.0.30")));
        assert!(!grok_image_compatibility("grok", Some("1.0.31")));
        assert!(supports_prompt_images(
            &serde_json::json!({"agentCapabilities":{"promptCapabilities":{"image":true}}}),
            false
        ));
    }
    #[tokio::test]
    async fn codebuddy_reference_name_retains_the_full_path_for_native_mentions() {
        let item = agent_domain::PromptAttachment {
            integrity: None,
            name: "reference #1.txt".into(),
            path: std::env::temp_dir()
                .join("attachments/reference #1.txt")
                .to_string_lossy()
                .into(),
            mime_type: "text/plain".into(),
        };
        let blocks = attachment_blocks("codebuddy", std::slice::from_ref(&item))
            .await
            .unwrap();
        let wire = serde_json::to_value(blocks).unwrap();
        assert_eq!(wire[0]["name"], item.path);
        assert_eq!(wire[0]["type"], "resource_link");
        assert!(wire[0]["uri"].as_str().unwrap().contains("%23"));
        assert!(wire[0].get("data").is_none());
    }
    #[tokio::test]
    async fn resource_links_preserve_spaces_unicode_and_hash_in_paths() {
        let path = std::env::temp_dir()
            .join("设计 #1.txt")
            .to_string_lossy()
            .into_owned();
        let blocks = attachment_blocks(
            "kimi",
            &[agent_domain::PromptAttachment {
                integrity: None,
                name: "设计 #1.txt".into(),
                path: path.clone(),
                mime_type: "text/plain".into(),
            }],
        )
        .await
        .unwrap();
        let wire = serde_json::to_value(&blocks).unwrap();
        assert_eq!(wire[0]["type"], "resource_link");
        let uri = url::Url::parse(wire[0]["uri"].as_str().unwrap()).unwrap();
        assert_eq!(uri.to_file_path().unwrap(), PathBuf::from(path));
        assert!(uri.fragment().is_none());
    }
}

fn uuid_question_id() -> String {
    // Approval/question ids outlive individual connections in UI/recovery
    // records. A resetting connection or process counter can alias a new one.
    agent_domain::InstallationId::new().to_string()
}

#[cfg(test)]
mod new_agents_tests;
