//! Production-facing Codex App Server adapter.
//!
//! The adapter owns one bounded stdio process per native Thread. It never reads Codex credential
//! files or accepts API keys. Authentication remains entirely inside the user's Codex runtime.

#[cfg(all(test, unix))]
mod cleanup_tests;
#[cfg(test)]
mod fault_recovery_tests;
mod lifecycle;
mod metadata;
#[cfg(all(test, unix))]
mod metadata_tests;
mod session_scope;
use session_scope::{
    ResidentSession, Scope, ScopedCommand, SessionSender, next_command, stale_scope,
};
mod live_usage;
mod request_failure;
mod request_usage;
mod rollout_usage;
#[cfg(test)]
mod transport_recovery_tests;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use agent_domain::{ExecutorRef, ProviderId};
use async_trait::async_trait;
use driver_api::{
    AgentDriver, ContextDeliveryKind, ContextUsageSnapshot, DriverError, DriverEvent,
    DriverEventStream, PromptRequest, QuotaProvider, QuotaSnapshot, QuotaWindow, SessionHandle,
    SessionRequest, SessionResumeCapability,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{Mutex, mpsc, oneshot, watch},
    time::timeout,
};
use tokio_stream::wrappers::ReceiverStream;

const MAX_FRAME_BYTES: usize = 64 * 1024;
// A reviewed context package may be 1 MiB before JSON escaping. Native user-message
// notifications echo that content, so both directions need the larger bounded budget.
const MAX_RPC_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_INTERLEAVED_FRAMES: usize = 256;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const QUOTA_PARSER_VERSION: &str = "codex-app-server-v1";
const QUOTA_TTL_MS: u64 = 15_000;
const QUOTA_TTL_MS_I64: i64 = 15_000;

mod file_approval;

struct ParsedRateLimits {
    windows: Vec<QuotaWindow>,
    plan_type: Option<String>,
}

/// Parses the account-level `rateLimits` shape (codex-host verified):
/// `{ primary?, secondary?, rateLimitsByLimitId? }` with windows
/// `{ usedPercent, windowDurationMins, resetsAt }`. Model-specific buckets in
/// `rateLimitsByLimitId` are excluded by construction — only the account-wide
/// windows belong in a quota snapshot.
fn parse_rate_limits(value: Option<&Value>) -> Option<ParsedRateLimits> {
    let response_or_snapshot = value?;
    let rate_limits = response_or_snapshot
        .get("rateLimits")
        .unwrap_or(response_or_snapshot);
    if rate_limits
        .get("limitId")
        .is_some_and(|value| !value.is_null() && value.as_str() != Some("codex"))
    {
        return None;
    }
    let mut windows = Vec::new();
    for (key, id) in [("primary", "primary"), ("secondary", "secondary")] {
        let Some(window) = rate_limits.get(key) else {
            continue;
        };
        let used_percent = window.get("usedPercent").and_then(Value::as_f64);
        let window_minutes = window.get("windowDurationMins").and_then(Value::as_u64);
        if let (Some(used_percent), Some(window_minutes)) = (used_percent, window_minutes)
            && (0.0..=100.0).contains(&used_percent)
            && window_minutes > 0
        {
            windows.push(QuotaWindow {
                scope: None,
                id: id.into(),
                window_minutes,
                used_percent: Some(used_percent),
                resets_at_unix: window.get("resetsAt").and_then(Value::as_i64),
            });
        }
    }
    windows.sort_by_key(|window| window.window_minutes);
    (!windows.is_empty()).then(|| ParsedRateLimits {
        windows,
        plan_type: rate_limits
            .get("planType")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Shared quota state: the pull cache (with freshness TTL) plus live context
/// usage observed from token-usage notifications.
#[derive(Default)]
struct QuotaState {
    usage: driver_api::usage::UsageStore,
    rollouts: std::sync::Mutex<HashMap<String, (PathBuf, Option<String>)>>,
    rollout_readers:
        std::sync::Mutex<HashMap<String, Arc<std::sync::Mutex<rollout_usage::Reader>>>>,
    snapshot: std::sync::Mutex<Option<QuotaSnapshot>>,
    fresh_until_unix_ms: std::sync::atomic::AtomicU64,
    connection: tokio::sync::Mutex<Option<Connection>>,
    context_by_thread: std::sync::Mutex<HashMap<String, ContextUsageSnapshot>>,
}

impl QuotaState {
    fn observe_thread(&self, id: &str, response: &Value) {
        if let Some(path) = response.pointer("/thread/path").and_then(Value::as_str) {
            self.rollouts
                .lock()
                .unwrap()
                .insert(id.into(), (path.into(), None));
        }
        if let Some(model) = response
            .pointer("/thread/model")
            .or_else(|| response.get("model"))
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
        {
            self.usage.merge(
                id,
                driver_api::usage::SessionUsageSnapshot {
                    model: Some(model.into()),
                    ..driver_api::usage::SessionUsageSnapshot::new()
                },
            );
        }
    }
    /// Pull/push arbitration, following the codex-host rules: pull is
    /// authoritative (15s TTL); a push fills an empty snapshot or invalidates
    /// the pull cache so the next read refreshes without showing mixed times.
    fn record(&self, snapshot: QuotaSnapshot, source: &str) {
        let mut current = self.snapshot.lock().unwrap();
        if source == "native_push" && current.is_some() {
            if let Some(snapshot) = current.as_mut() {
                snapshot.stale_after_unix_ms = now_unix_ms_i64();
            }
            self.fresh_until_unix_ms
                .store(0, std::sync::atomic::Ordering::Release);
            return;
        }
        *current = Some(snapshot);
        drop(current);
        if source == "native_pull" {
            self.fresh_until_unix_ms.store(
                now_unix_ms().saturating_add(QUOTA_TTL_MS),
                std::sync::atomic::Ordering::Release,
            );
        }
    }

    /// Captures `account/rateLimits/updated` (push arbitration) and
    /// `thread/tokenUsage/updated` (context window + cache hit rate).
    fn observe_notification(&self, method: &str, params: &Value) {
        match method {
            "account/rateLimits/updated" => {
                if let Some(parsed) = parse_rate_limits(params.get("rateLimits")) {
                    let collected_at_unix_ms = now_unix_ms_i64();
                    self.record(
                        QuotaSnapshot {
                            balances: Vec::new(),
                            session_usage: None,
                            windows: parsed.windows,
                            context: None,
                            collected_at_unix_ms,
                            stale_after_unix_ms: collected_at_unix_ms,
                            plan_type: parsed.plan_type,
                            source: "native_push".into(),
                            confidence: "provider_reported".into(),
                            parser_version: QUOTA_PARSER_VERSION.into(),
                        },
                        "native_push",
                    );
                }
            }
            "thread/tokenUsage/updated" => {
                if let Some(id) = params["threadId"].as_str() {
                    self.usage
                        .set(id, driver_api::usage::codex(&params["tokenUsage"]));
                }
                let token_usage = params.get("tokenUsage");
                let used = token_usage
                    .and_then(|usage| usage.get("last"))
                    .and_then(|last| last.get("totalTokens"))
                    .and_then(Value::as_u64);
                let window = token_usage
                    .and_then(|usage| usage.get("modelContextWindow"))
                    .and_then(Value::as_u64);
                if let (Some(used), Some(window)) = (used, window)
                    && window > 0
                {
                    let total = token_usage.and_then(|usage| usage.get("last"));
                    let input = total
                        .and_then(|total| total.get("inputTokens"))
                        .and_then(Value::as_u64);
                    let cached = total
                        .and_then(|total| total.get("cachedInputTokens"))
                        .and_then(Value::as_u64);
                    let cache_hit_rate_percent = match (input, cached) {
                        (Some(input), Some(cached)) => {
                            driver_api::usage::ratio(Some(cached), Some(input))
                        }
                        _ => None,
                    };
                    if let Some(thread_id) = params.get("threadId").and_then(Value::as_str) {
                        self.context_by_thread.lock().unwrap().insert(
                            thread_id.to_owned(),
                            ContextUsageSnapshot {
                                used_tokens: used,
                                window_tokens: window,
                                cache_hit_rate_percent,
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn fresh(&self) -> bool {
        now_unix_ms()
            < self
                .fresh_until_unix_ms
                .load(std::sync::atomic::Ordering::Acquire)
    }

    fn snapshot_for(&self, native_session_id: Option<&str>) -> Option<QuotaSnapshot> {
        let mut snapshot = self.snapshot.lock().unwrap().clone()?;
        snapshot.context = native_session_id.and_then(|thread_id| {
            self.context_by_thread
                .lock()
                .unwrap()
                .get(thread_id)
                .cloned()
        });
        Some(snapshot)
    }

    fn quota_failure(
        &self,
        cached: Option<QuotaSnapshot>,
        error: DriverError,
    ) -> Result<QuotaSnapshot, DriverError> {
        if driver_api::failure::invalidates_account_quota(&error) {
            *self.snapshot.lock().unwrap() = None;
            return Err(error);
        }
        cached.ok_or(error)
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

fn now_unix_ms_i64() -> i64 {
    i64::try_from(now_unix_ms()).unwrap_or(i64::MAX)
}

pub struct CodexDriver {
    executor: ExecutorRef,
    executable: PathBuf,
    native_agent_version: String,
    sessions: Arc<Mutex<BTreeMap<String, ResidentSession>>>,
    stopping: watch::Sender<bool>,
    retired_handles: std::sync::Mutex<BTreeMap<String, SessionHandle>>,
    restore_gates: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    quota: Arc<QuotaState>,
    catalog_probe: Arc<Mutex<()>>,
    history_probe: Arc<Mutex<()>>,
}

impl Drop for CodexDriver {
    fn drop(&mut self) {
        self.stopping.send_replace(true);
    }
}

impl CodexDriver {
    pub fn new(
        executable: &Path,
        installation_id: String,
        native_agent_version: String,
    ) -> Result<Self, DriverError> {
        let executable = std::fs::canonicalize(executable)
            .map_err(|_| stable_error("codex executable is unavailable"))?;
        if !executable.is_file() || native_agent_version.trim().is_empty() {
            return Err(stable_error("codex installation is invalid"));
        }
        Ok(Self {
            executor: ExecutorRef {
                provider_id: ProviderId::parse("codex")
                    .map_err(|_| stable_error("codex provider identity is invalid"))?,
                installation_id: Some(installation_id),
            },
            executable,
            native_agent_version,
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            stopping: watch::channel(false).0,
            retired_handles: std::sync::Mutex::new(BTreeMap::new()),
            restore_gates: std::sync::Mutex::new(HashMap::new()),
            quota: Arc::new(QuotaState::default()),
            catalog_probe: Arc::new(Mutex::new(())),
            history_probe: Arc::new(Mutex::new(())),
        })
    }

    async fn remove_closed_session(&self, id: &str, sender: &ResidentSession) {
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(id)
            .is_some_and(|current| current.commands.same_channel(&sender.commands))
            && let Some(retired) = sessions.remove(id)
        {
            self.retired_handles
                .lock()
                .expect("Codex retired scopes")
                .insert(id.into(), retired.handle());
        }
    }

    /// Performs initialize + account/read without starting a Thread or model Turn.
    pub async fn probe_ready(executable: &Path) -> Result<(), DriverError> {
        let executable = std::fs::canonicalize(executable)
            .map_err(|_| stable_error("codex executable is unavailable"))?;
        let mut connection = Connection::spawn(&executable).await?;
        let result = connection
            .request("account/read", json!({ "refreshToken": false }))
            .await;
        let ready = result.as_ref().is_ok_and(|exchange| {
            exchange.result.get("account").is_some_and(Value::is_object)
                || exchange.result.get("requiresOpenaiAuth") == Some(&Value::Bool(false))
        });
        connection.shutdown().await?;
        match result {
            Ok(_) if ready => Ok(()),
            Ok(exchange) => Err(DriverError::NotConfigured(
                if exchange.result["requiresOpenaiAuth"] == true {
                    "authentication_failed"
                } else {
                    "driver_not_configured"
                }
                .into(),
            )),
            Err(error) => Err(error),
        }
    }

    async fn open_thread(
        &self,
        request: &SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        let working_directory = Path::new(&request.working_directory);
        if !working_directory.is_absolute() || !working_directory.is_dir() {
            return Err(stable_error("working directory is unavailable"));
        }
        let mut connection = Connection::spawn(&self.executable).await?;
        connection.quota = Some(Arc::clone(&self.quota));
        let full_review = request.permission_mode.as_deref() == Some("hub-review");
        let analysis = full_review || request.permission_mode.as_deref() == Some("hub-analysis");
        connection.full_review = full_review;
        connection.analysis_only = analysis;
        let mut config = question_session_config(request.history_mcp.as_ref(), thinking);
        if analysis {
            // Disable inherited remote tools, while allowing sandboxed local reads. Never guess
            // an empty config means configured MCP servers were removed.
            let effective = connection
                .request("config/read", json!({"includeLayers":false}))
                .await;
            let effective = match effective {
                Ok(value)
                    if value.result.get("config").is_some_and(Value::is_object)
                        && value
                            .result
                            .pointer("/config/mcp_servers")
                            .is_none_or(|v| v.is_null() || v.is_object()) =>
                {
                    value
                }
                _ => {
                    connection.shutdown().await?;
                    return Err(stable_error(
                        "cannot verify isolated analysis configuration",
                    ));
                }
            };
            for section in ["mcp_servers", "apps"] {
                if let Some(entries) = effective.result["config"].get(section)
                    && !entries.is_null()
                    && (!entries.is_object()
                        || entries.as_object().is_some_and(|o| {
                            o.keys().any(|name| {
                                name.is_empty()
                                    || !name.bytes().all(|b| {
                                        b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
                                    })
                            })
                        }))
                {
                    connection.shutdown().await?;
                    return Err(stable_error(
                        "cannot safely override inherited analysis tools",
                    ));
                }
            }
            if let Some(servers) = effective
                .result
                .pointer("/config/mcp_servers")
                .and_then(Value::as_object)
            {
                for name in servers.keys() {
                    config[format!("mcp_servers.{name}.enabled")] = json!(false);
                }
            }
            if let Some(apps) = effective
                .result
                .pointer("/config/apps")
                .and_then(Value::as_object)
            {
                for name in apps.keys() {
                    config[format!("apps.{name}.enabled")] = json!(false);
                }
            }
            config["features.shell_tool"] = json!(true);
            config["features.multi_agent"] = json!(false);
            config["apps._default.enabled"] = json!(false);
            config["web_search"] = json!("disabled");
        }
        let exchange = connection
            .request(
                "thread/start",
                json!({
                    "cwd": request.working_directory,
                    "approvalPolicy": if analysis { "never" } else { "untrusted" },
                    "sandbox": if full_review { "danger-full-access" } else if analysis { "read-only" } else { "workspace-write" },
                    "ephemeral": analysis,
                    "experimentalRawEvents": true,
                    "model": model,
                    "config": config
                }),
            )
            .await;
        let exchange = match exchange {
            Ok(exchange) => exchange,
            Err(error) => {
                connection.shutdown().await?;
                return Err(error);
            }
        };
        let native_session_id = exchange
            .result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let Some(native_session_id) = native_session_id else {
            connection.shutdown().await?;
            return Err(stable_error("codex thread/start response is invalid"));
        };
        connection.thread_id.clone_from(&native_session_id);
        self.quota
            .observe_thread(&native_session_id, &exchange.result);
        connection.quota = Some(Arc::clone(&self.quota));
        self.install_session(native_session_id, connection).await
    }

    async fn install_session(
        &self,
        native_session_id: String,
        connection: Connection,
    ) -> Result<SessionHandle, DriverError> {
        let handle = SessionHandle::scoped(native_session_id.clone());
        let scope = Arc::new(std::sync::Mutex::new(handle.clone()));
        let (sender, receiver) = mpsc::channel(4);
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(&native_session_id)
            .is_some_and(|sender| !sender.is_closed())
        {
            return Err(stable_error("codex session is already active"));
        }
        if let Some(old) = sessions.get(&native_session_id)
            && let Some(native) = &old.native
        {
            native.request_close();
            native.closed().await?;
        }
        connection.native.observe_owner(self.stopping.subscribe());
        sessions.insert(
            native_session_id,
            ResidentSession {
                commands: sender,
                native: Some(connection.native.clone()),
                scope: scope.clone(),
            },
        );
        drop(sessions);
        tokio::spawn(session_actor(connection, receiver, scope));
        Ok(handle)
    }

    async fn command_sender(&self, handle: &SessionHandle) -> Result<SessionSender, DriverError> {
        let sessions = self.sessions.lock().await;
        let resident = sessions
            .get(&handle.native_session_id)
            .ok_or_else(|| DriverError::SessionMissing("codex session is not active".into()))?;
        if !resident.handle().same_scope(handle) {
            return Err(stale_scope());
        }
        Ok(resident.sender_for(handle.clone()))
    }
}

#[async_trait]
impl AgentDriver for CodexDriver {
    fn supports_readonly_analysis(&self) -> bool {
        true
    }
    async fn create_analysis_session(&self, directory: &str) -> Result<SessionHandle, DriverError> {
        self.create_analysis_session_with_directories(directory, &[])
            .await
    }
    async fn create_analysis_session_with_directories(
        &self,
        directory: &str,
        additional: &[String],
    ) -> Result<SessionHandle, DriverError> {
        self.open_thread(
            &SessionRequest {
                working_directory: directory.into(),
                additional_directories: additional.to_vec(),
                history_mcp: None,
                permission_mode: Some("hub-analysis".into()),
            },
            None,
            None,
        )
        .await
    }

    async fn create_review_session(
        &self,
        directory: &str,
        additional: &[String],
    ) -> Result<SessionHandle, DriverError> {
        self.open_thread(
            &SessionRequest {
                working_directory: directory.into(),
                additional_directories: additional.to_vec(),
                history_mcp: None,
                permission_mode: Some("hub-review".into()),
            },
            None,
            None,
        )
        .await
    }

    async fn permission_catalog(
        &self,
        _workspace: Option<&str>,
    ) -> Result<driver_api::PermissionCatalog, DriverError> {
        Ok(driver_api::permissions::codex())
    }
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }

    fn driver_version(&self) -> &'static str {
        DRIVER_VERSION
    }

    fn native_agent_version(&self) -> Option<&str> {
        Some(&self.native_agent_version)
    }

    fn supports_live_session_reuse(&self) -> bool {
        self.native_agent_version != "0.144.5"
    }

    fn resume_capability(&self) -> SessionResumeCapability {
        if matches!(self.native_agent_version.as_str(), "0.144.5" | "0.153.4") {
            SessionResumeCapability::Supported
        } else {
            SessionResumeCapability::Unknown
        }
    }

    async fn read_subagent(&self, id: &str) -> Result<Vec<Value>, DriverError> {
        let history = metadata::read_history(self, id).await?;
        let mut output = Vec::new();
        if let Some(turns) = history.pointer("/thread/turns").and_then(Value::as_array) {
            for turn in turns {
                if let Some(items) = turn["items"].as_array() {
                    for item in items {
                        if item["type"] == "agentMessage" {
                            output.push(json!({"type":"message","text":item["text"]}));
                        }
                        for event in driver_api::presentation::presentation_events(
                            "codex",
                            "item/completed",
                            &json!({"item":item}),
                        ) {
                            if let DriverEvent::Presentation { data, .. } = event {
                                output.push(data);
                            }
                        }
                    }
                }
            }
        }
        if let Some(turn) = history
            .pointer("/thread/turns")
            .and_then(Value::as_array)
            .and_then(|turns| turns.last())
        {
            output.push(json!({"type":"status","status":turn["status"]}));
        }
        Ok(output)
    }

    async fn model_catalog(&self) -> Result<driver_api::ModelCatalog, DriverError> {
        metadata::read(self).await
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.open_thread(&request, model, thinking).await
    }
    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError> {
        self.open_thread(&request, None, None).await
    }

    async fn resume_session(
        &self,
        native_session_id: &str,
        working_directory: &str,
    ) -> Result<SessionHandle, DriverError> {
        let _ = working_directory;
        self.resume_selected_session(
            native_session_id,
            SessionRequest {
                history_mcp: None,
                permission_mode: None,
                working_directory: working_directory.into(),
                additional_directories: vec![],
            },
            None,
            None,
        )
        .await
    }
    async fn reconfigure_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        let session = self
            .resume_selected_session(id, request, model, thinking)
            .await?;
        let (response, reply) = oneshot::channel();
        self.command_sender(&session)
            .await?
            .send(SessionCommand::Configure {
                model: model.map(str::to_owned),
                thinking: thinking.map(str::to_owned),
                response,
            })
            .await
            .map_err(|_| DriverError::Disconnected("codex configuration channel closed".into()))?;
        reply
            .await
            .map_err(|_| DriverError::Disconnected("codex configuration reply closed".into()))??;
        Ok(session)
    }

    async fn resume_selected_session(
        &self,
        native_session_id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        if native_session_id.is_empty() {
            return Err(DriverError::SessionMissing(
                "codex session identity is missing".into(),
            ));
        }
        let gate = self
            .restore_gates
            .lock()
            .unwrap()
            .entry(native_session_id.into())
            .or_default()
            .clone();
        let _restoring = gate.lock().await;
        let existing = self.sessions.lock().await.get(native_session_id).cloned();
        if let Some(sender) = existing {
            let lease = sender.sender();
            let next_scope = SessionHandle::scoped(native_session_id.into());
            if !sender.is_closed() {
                let (response, received) = oneshot::channel();
                if lease
                    .send(SessionCommand::CheckReady {
                        response,
                        next_scope: next_scope.clone(),
                    })
                    .await
                    .is_ok()
                {
                    match timeout(SHUTDOWN_TIMEOUT, received).await {
                        Ok(Ok(Ok(()))) => {
                            return Ok(next_scope);
                        }
                        Ok(Ok(Err(DriverError::CapabilityUnavailable(reason)))) => {
                            return Err(DriverError::CapabilityUnavailable(reason));
                        }
                        Ok(Ok(Err(error @ DriverError::SessionIncompatible(_)))) => {
                            return Err(error);
                        }
                        Err(_) => {
                            return Err(DriverError::Disconnected(
                                "codex session health check timed out".into(),
                            ));
                        }
                        _ => {}
                    }
                }
            }
            if let Some(native) = &sender.native {
                native.request_close();
                native.closed().await?;
            }
            self.remove_closed_session(native_session_id, &sender).await;
        }
        let mut connection = Connection::spawn(&self.executable).await?;
        let exchange = connection
            .request("thread/resume", json!({ "threadId": native_session_id, "model": model, "config": question_session_config(request.history_mcp.as_ref(), thinking) }))
            .await;
        let exchange = match exchange {
            Ok(exchange) => exchange,
            Err(DriverError::Protocol(_)) => {
                connection.shutdown().await?;
                return Err(DriverError::SessionMissing(
                    "codex native session could not be resumed".into(),
                ));
            }
            Err(error) => {
                connection.shutdown().await?;
                return Err(error);
            }
        };
        let resumed_id = exchange
            .result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(resumed_id) = resumed_id else {
            connection.shutdown().await?;
            return Err(DriverError::SessionIncompatible(
                "codex resume response is invalid".into(),
            ));
        };
        if resumed_id != native_session_id {
            connection.shutdown().await?;
            return Err(DriverError::SessionIncompatible(
                "codex resumed a different native session".into(),
            ));
        }
        connection.thread_id = native_session_id.into();
        self.quota
            .observe_thread(native_session_id, &exchange.result);
        connection.quota = Some(Arc::clone(&self.quota));
        self.install_session(native_session_id.into(), connection)
            .await
    }

    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        let sender = self.command_sender(handle).await?;
        let (response_tx, response_rx) = oneshot::channel();
        sender
            .send(SessionCommand::Prompt {
                request,
                response: response_tx,
            })
            .await
            .map_err(|_| DriverError::Disconnected("codex session actor stopped".into()))?;
        let receiver = response_rx
            .await
            .map_err(|_| DriverError::Disconnected("codex session actor stopped".into()))??;
        Ok(Box::pin(ReceiverStream::new(receiver)))
    }

    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        let sender = self.command_sender(handle).await?;
        let (response_tx, response_rx) = oneshot::channel();
        sender
            .send(SessionCommand::Interrupt {
                response: response_tx,
            })
            .await
            .map_err(|_| DriverError::Disconnected("codex session actor stopped".into()))?;
        response_rx
            .await
            .map_err(|_| DriverError::Disconnected("codex session actor stopped".into()))?
    }

    async fn respond_to_question(
        &self,
        handle: &SessionHandle,
        request_id: &str,
        answer: agent_domain::QuestionAnswer,
    ) -> Result<(), DriverError> {
        let sender = self.command_sender(handle).await?;
        let (response, response_rx) = oneshot::channel();
        sender
            .send(SessionCommand::Answer {
                native_request_id: request_id.into(),
                answer,
                response,
            })
            .await
            .map_err(|_| DriverError::Disconnected("Codex closed".into()))?;
        response_rx
            .await
            .map_err(|_| DriverError::Disconnected("Codex closed".into()))?
    }

    async fn respond_to_interaction(
        &self,
        handle: &SessionHandle,
        native_request_id: &str,
        option_id: &str,
    ) -> Result<(), DriverError> {
        let sender = self.command_sender(handle).await?;
        let (response_tx, response_rx) = oneshot::channel();
        sender
            .send(SessionCommand::Approve {
                native_request_id: native_request_id.to_owned(),
                option_id: option_id.to_owned(),
                response: response_tx,
            })
            .await
            .map_err(|_| DriverError::Disconnected("codex session actor stopped".into()))?;
        response_rx
            .await
            .map_err(|_| DriverError::Disconnected("codex session actor stopped".into()))?
    }

    async fn current_session_handle(&self, id: &str) -> Option<SessionHandle> {
        self.sessions
            .lock()
            .await
            .get(id)
            .map(ResidentSession::handle)
    }

    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        let sessions = self.sessions.lock().await;
        if let Some(resident) = sessions.get(&handle.native_session_id) {
            if !resident.handle().same_scope(handle) {
                return None;
            }
            let native = resident.native.clone()?;
            return Some(Box::pin(async move { native.confirmed().await }));
        }
        self.retired_handles
            .lock()
            .expect("Codex retired scopes")
            .get(&handle.native_session_id)
            .filter(|retired| retired.same_scope(handle))
            .map(|_| Box::pin(async {}) as driver_api::SessionShutdownProof)
    }

    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        // Capture authority and native resources once. A cancelled/timed-out
        // observer must not mistake map absence or a replacement for retirement.
        let resident = {
            let sessions = self.sessions.lock().await;
            if let Some(resident) = sessions.get(&handle.native_session_id) {
                if !resident.handle().same_scope(&handle) {
                    return Err(stale_scope());
                }
                resident.clone()
            } else {
                return if self
                    .retired_handles
                    .lock()
                    .expect("Codex retired scopes")
                    .get(&handle.native_session_id)
                    .is_some_and(|retired| retired.same_scope(&handle))
                {
                    Ok(())
                } else {
                    Err(stale_scope())
                };
            }
        };
        let (response_tx, response_rx) = oneshot::channel();
        let sent = resident
            .sender_for(handle.clone())
            .send(SessionCommand::Shutdown {
                response: response_tx,
            })
            .await
            .is_ok();
        if sent {
            match timeout(SHUTDOWN_TIMEOUT, response_rx).await {
                Ok(Ok(result)) => result?,
                Ok(Err(_)) if resident.native.is_some() => {}
                Ok(Err(_)) => {
                    return Err(DriverError::Disconnected(
                        "codex session actor stopped without shutdown confirmation".into(),
                    ));
                }
                Err(_) => return Err(DriverError::Disconnected("codex shutdown timed out".into())),
            }
        } else if resident.native.is_none() {
            return Err(DriverError::Disconnected(
                "codex native exit remains unconfirmed".into(),
            ));
        }
        if let Some(native) = &resident.native {
            native.request_close();
            native.closed().await?;
        }
        self.remove_closed_session(&handle.native_session_id, &resident)
            .await;
        Ok(())
    }
}

#[async_trait]
impl QuotaProvider for CodexDriver {
    async fn read_session_usage(
        &self,
        id: &str,
    ) -> Option<driver_api::usage::SessionUsageSnapshot> {
        let mut usage = self.quota.usage.get(Some(id)).unwrap_or_default();
        let binding = self.quota.rollouts.lock().unwrap().get(id).cloned();
        if let Some((path, turn)) = binding {
            let thread = id.to_owned();
            let reader = self
                .quota
                .rollout_readers
                .lock()
                .unwrap()
                .entry(id.into())
                .or_default()
                .clone();
            let native_turn = turn.clone();
            if let Ok(Some(history)) = tokio::task::spawn_blocking(move || {
                reader
                    .lock()
                    .unwrap()
                    .read(&path, &thread, native_turn.as_deref())
            })
            .await
            {
                // Merge under the store lock: live observations may arrive during disk I/O.
                usage = self.quota.usage.update(id, |usage| {
                    // A partial scan cannot replace an entire turn's checkpoint ledger.
                    // Defer exact facts for that turn to avoid counting both representations.
                    let defer = !history.caught_up
                        && turn.as_ref().is_some_and(|turn| {
                            usage.request_usage.as_ref().is_some_and(|report| {
                                report.records.iter().any(|r| {
                                    r.request_id.starts_with(&format!("checkpoint:{turn}:"))
                                })
                            })
                        });
                    if !defer {
                        let report = usage.request_usage.get_or_insert_with(Default::default);
                        report.truncated |= history.report.truncated;
                        report.records.retain(|r| {
                            !history
                                .exact_turns
                                .iter()
                                .any(|t| r.request_id.starts_with(&format!("checkpoint:{t}:")))
                        });
                        for mut record in history.report.records {
                            if let Some(live) = report
                                .records
                                .iter()
                                .find(|r| r.request_id == record.request_id)
                            {
                                record.generation_ms = live.generation_ms;
                                record.source.clone_from(&live.source);
                                record.model = record.model.or_else(|| live.model.clone());
                            }
                            if report.records.len() >= 10_000
                                && !report
                                    .records
                                    .iter()
                                    .any(|r| r.request_id == record.request_id)
                            {
                                report.records.remove(0);
                                report.truncated = true;
                            }
                            report.observe(record);
                        }
                    }
                    if let Some(model) = history.model {
                        usage.model = Some(model.clone());
                        if let (Some(turn), Some(report)) = (&turn, &mut usage.request_usage) {
                            for record in &mut report.records {
                                if record
                                    .request_id
                                    .starts_with(&format!("checkpoint:{turn}:"))
                                {
                                    record.model = Some(model.clone());
                                }
                            }
                        }
                    }
                });
            }
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
        let stale_snapshot = if force {
            None
        } else {
            self.quota.snapshot_for(native_session_id)
        };
        if !force
            && self.quota.fresh()
            && let Some(snapshot) = stale_snapshot.clone()
        {
            return Ok(snapshot);
        }
        // Serialize quota pulls onto one dedicated app-server connection; a
        // broken connection is dropped and respawned on the next attempt.
        let mut guard = self.quota.connection.lock().await;
        if let Some(original) = guard.as_mut()
            && original.native.stopping()
        {
            if let Err(error) = original.shutdown().await {
                return self.quota.quota_failure(stale_snapshot, error);
            }
            *guard = None;
        }
        if guard.is_none() {
            let connection = match Connection::spawn(&self.executable).await {
                Ok(connection) => connection,
                Err(error) => return self.quota.quota_failure(stale_snapshot, error),
            };
            // This helper is already owned by QuotaState. Pointing back to it
            // would retain both the cache and native CLI after Driver Drop.
            connection.native.observe_owner(self.stopping.subscribe());
            *guard = Some(connection);
        }
        let connection = guard.as_mut().unwrap();
        let exchange = connection
            .request("account/rateLimits/read", json!({}))
            .await;
        let exchange = match exchange {
            Ok(exchange) => exchange,
            Err(error) => {
                let cleanup = connection.shutdown().await;
                if cleanup.is_ok() {
                    *guard = None;
                }
                return self
                    .quota
                    .quota_failure(stale_snapshot, cleanup.err().unwrap_or(error));
            }
        };
        for frame in &exchange.interleaved {
            if let IncomingFrame::Notification { method, params } = frame
                && matches!(
                    method.as_str(),
                    "account/rateLimits/updated" | "thread/tokenUsage/updated"
                )
            {
                self.quota.observe_notification(method, params);
            }
        }
        let Some(parsed) = parse_rate_limits(Some(&exchange.result)) else {
            return stale_snapshot.ok_or_else(|| {
                DriverError::Protocol(
                    "codex rate limit response did not contain account windows".into(),
                )
            });
        };
        let collected_at_unix_ms = now_unix_ms_i64();
        let snapshot = QuotaSnapshot {
            balances: Vec::new(),
            session_usage: None,
            windows: parsed.windows,
            context: None,
            collected_at_unix_ms,
            stale_after_unix_ms: collected_at_unix_ms.saturating_add(QUOTA_TTL_MS_I64),
            plan_type: parsed.plan_type,
            source: "native_pull".into(),
            confidence: "provider_reported".into(),
            parser_version: QUOTA_PARSER_VERSION.into(),
        };
        self.quota.record(snapshot, "native_pull");
        self.quota
            .snapshot_for(native_session_id)
            .ok_or_else(|| DriverError::Protocol("codex quota snapshot could not be cached".into()))
    }
}

enum SessionCommand {
    CheckReady {
        next_scope: SessionHandle,
        response: oneshot::Sender<Result<(), DriverError>>,
    },
    Answer {
        native_request_id: String,
        answer: agent_domain::QuestionAnswer,
        response: oneshot::Sender<Result<(), DriverError>>,
    },
    Configure {
        model: Option<String>,
        thinking: Option<String>,
        response: oneshot::Sender<Result<(), DriverError>>,
    },
    Prompt {
        request: PromptRequest,
        response:
            oneshot::Sender<Result<mpsc::Receiver<Result<DriverEvent, DriverError>>, DriverError>>,
    },
    Interrupt {
        response: oneshot::Sender<Result<(), DriverError>>,
    },
    Approve {
        native_request_id: String,
        option_id: String,
        response: oneshot::Sender<Result<(), DriverError>>,
    },
    Shutdown {
        response: oneshot::Sender<Result<(), DriverError>>,
    },
}

enum ActorControl {
    Continue,
    Stop,
    Shutdown(oneshot::Sender<Result<(), DriverError>>),
}

async fn session_actor(
    mut connection: Connection,
    mut commands: mpsc::Receiver<ScopedCommand>,
    scope: Scope,
) {
    let mut shutdown_response = None;
    while let Some(command) = next_command(&mut commands, &scope).await {
        match command {
            SessionCommand::CheckReady {
                response,
                next_scope,
            } => {
                if connection.native.stopping() {
                    // Confirm cleanup before allowing the caller to restore this identity.
                    let _ = connection.shutdown().await;
                    let _ = response.send(Err(DriverError::Disconnected(
                        "codex idle process exited".into(),
                    )));
                    break;
                }
                let mut current = scope.lock().expect("Codex session scope");
                if response.send(Ok(())).is_ok() {
                    *current = next_scope;
                }
            }
            SessionCommand::Configure {
                model,
                thinking,
                response,
            } => {
                let result = connection.configure_selection(model, thinking).await;
                let _ = response.send(result);
            }
            SessionCommand::Prompt { request, response } => {
                if response.is_closed() {
                    continue;
                }
                let (events_tx, events_rx) = mpsc::channel(64);
                match start_turn(&mut connection, request).await {
                    Ok(started) => {
                        if response.send(Ok(events_rx)).is_err() {
                            break;
                        }
                        match run_turn(&mut connection, &mut commands, &scope, &events_tx, started)
                            .await
                        {
                            ActorControl::Continue => {}
                            ActorControl::Stop => break,
                            ActorControl::Shutdown(response) => {
                                shutdown_response = Some(response);
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        let disconnected = matches!(error, DriverError::Disconnected(_));
                        let _ = response.send(Err(error));
                        if disconnected {
                            break;
                        }
                    }
                }
            }
            SessionCommand::Interrupt { response }
            | SessionCommand::Answer { response, .. }
            | SessionCommand::Approve { response, .. } => {
                let _ = response.send(Err(DriverError::CapabilityUnavailable(
                    "codex has no active turn".into(),
                )));
            }
            SessionCommand::Shutdown { response } => {
                shutdown_response = Some(response);
                break;
            }
        }
    }
    let cleanup = connection.shutdown().await;
    if cleanup.is_ok()
        && let Some(response) = shutdown_response
    {
        let _ = response.send(Ok(()));
    }
}

struct StartedTurn {
    native_turn_id: String,
    working_directory: PathBuf,
    interleaved: Vec<IncomingFrame>,
}

#[derive(Default)]
struct InterruptWaiters(Vec<oneshot::Sender<Result<(), DriverError>>>);
impl InterruptWaiters {
    fn finish(&mut self, result: Result<(), DriverError>) {
        for response in self.0.drain(..) {
            let _ = response.send(result.clone());
        }
    }
}
impl Drop for InterruptWaiters {
    fn drop(&mut self) {
        self.finish(Err(DriverError::Disconnected(
            "Codex interruption has no confirmed terminal event".into(),
        )));
    }
}
/// A terminal `turn/completed` for this turn. The mapper ends the turn on any
/// such frame, so identities only disqualify it when present and different:
/// a frame without them (older app-server) still confirms the stop.
fn confirms_turn(frame: &IncomingFrame, thread_id: &str, turn_id: &str) -> bool {
    let IncomingFrame::Notification { method, params } = frame else {
        return false;
    };
    let thread = params["threadId"].as_str();
    let turn = params["turn"]["id"]
        .as_str()
        .or_else(|| params["turnId"].as_str());
    method == "turn/completed"
        && thread.is_none_or(|thread| thread == thread_id)
        && turn.is_none_or(|turn| turn == turn_id)
        && matches!(
            params["turn"]["status"].as_str(),
            Some("completed" | "interrupted" | "failed")
        )
}

async fn start_turn(
    connection: &mut Connection,
    request: PromptRequest,
) -> Result<StartedTurn, DriverError> {
    let mut input = Vec::with_capacity(request.context.len() + 1);
    for context in request.context {
        let content = String::from_utf8(context.content)
            .map_err(|_| stable_error("reviewed context is not UTF-8"))?;
        let kind = match context.kind {
            ContextDeliveryKind::Handoff => "handoff",
            ContextDeliveryKind::Reconciliation => "reconciliation",
        };
        input.push(json!({
            "type": "text",
            "text": format!(
                "dogcodes reviewed {kind} context (sha256={}):\n{content}",
                context.payload_sha256
            )
        }));
    }
    input.extend(driver_api::attachments::codex(&request.attachments).await?);
    if !request.prompt.is_empty() {
        input.push(json!({ "type": "text", "text": request.prompt }));
    }
    let thread_id = connection.thread_id.clone();
    let mut params = json!({ "threadId": thread_id, "input": input });
    if let Some((model, thinking)) = &connection.selection {
        params["model"] = json!(model);
        params["effort"] = json!(thinking);
    }
    if !request.additional_directories.is_empty() {
        let roots: Vec<_> = std::iter::once(&request.working_directory)
            .chain(&request.additional_directories)
            .collect();
        params["cwd"] = json!(request.working_directory);
        params["sandboxPolicy"] = json!({
            "type": "workspaceWrite", "writableRoots": roots,
            "networkAccess": false,
            "excludeTmpdirEnvVar": false, "excludeSlashTmp": false
        });
    }
    driver_api::permissions::codex().validate(request.permission_mode.as_deref())?;
    match request.permission_mode.as_deref().unwrap_or("default") {
        "full-access" => {
            params["approvalPolicy"] = json!("never");
            params["sandboxPolicy"] = json!({"type":"dangerFullAccess"});
        }
        "read-only" => {
            params["approvalPolicy"] = json!("on-request");
            params["sandboxPolicy"] = json!({"type":"readOnly"});
        }
        _ => {
            params["approvalPolicy"] = json!("untrusted");
            let roots: Vec<_> = std::iter::once(&request.working_directory)
                .chain(&request.additional_directories)
                .collect();
            params["sandboxPolicy"] = json!({"type":"workspaceWrite","writableRoots":roots,"networkAccess":false,"excludeTmpdirEnvVar":false,"excludeSlashTmp":false});
        }
    }
    if connection.analysis_only {
        params["approvalPolicy"] = json!("never");
        params["sandboxPolicy"] =
            json!({"type":if connection.full_review { "dangerFullAccess" } else { "readOnly" }});
    }
    let exchange = connection.request("turn/start", params).await?;
    let native_turn_id = exchange
        .result
        .get("turn")
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| stable_error("codex turn/start response is invalid"))?
        .to_owned();
    connection.live_usage.begin(&thread_id, &native_turn_id);
    if let Some(quota) = &connection.quota
        && let Some(binding) = quota.rollouts.lock().unwrap().get_mut(&thread_id)
    {
        binding.1 = Some(native_turn_id.clone());
    }
    Ok(StartedTurn {
        native_turn_id,
        working_directory: PathBuf::from(request.working_directory),
        interleaved: exchange.interleaved,
    })
}

async fn run_turn(
    connection: &mut Connection,
    commands: &mut mpsc::Receiver<ScopedCommand>,
    scope: &Scope,
    events: &mpsc::Sender<Result<DriverEvent, DriverError>>,
    started: StartedTurn,
) -> ActorControl {
    let mut terminal = false;
    let mut verified_terminal = false;
    let mut interrupts = InterruptWaiters::default();
    let mut stop_deadline = None;
    #[cfg(test)]
    let stop_timeout = connection.stop_timeout;
    #[cfg(not(test))]
    let stop_timeout = REQUEST_TIMEOUT;
    let mut pending_approvals: HashMap<String, Value> = HashMap::new();
    let mut next_approval_id = 1_u64;
    let mut file_changes = file_approval::FileChanges::new(
        connection.thread_id.clone(),
        started.native_turn_id.clone(),
    );
    file_changes.set_working_directory(started.working_directory);
    let _ = events
        .send(Ok(DriverEvent::TurnStarted {
            native_turn_id: Some(started.native_turn_id.clone()),
        }))
        .await;
    for frame in started.interleaved {
        verified_terminal |= confirms_turn(&frame, &connection.thread_id, &started.native_turn_id);
        terminal |= handle_frame(
            connection,
            events,
            frame,
            &mut pending_approvals,
            &mut next_approval_id,
            &mut file_changes,
        )
        .await;
    }
    while !terminal {
        tokio::select! {
            command = next_command(commands, scope) => match command {
                Some(SessionCommand::Answer {native_request_id,answer,response}) => {
                    let result = if let Some(pending)=pending_approvals.get(&native_request_id).filter(|p|p.get("questionId").is_some()) {
                        match driver_api::questions::native_form("codex", &pending["params"]).and_then(|form|form.validate(&answer)) {
                            Ok(()) => {
                                let pending=pending_approvals.remove(&native_request_id).expect("pending question");
                                connection.transport.send_result(pending["questionId"].clone(),driver_api::questions::native_answer("codex",&pending["params"],&answer)).await
                            },
                            Err(error)=>Err(DriverError::Protocol(error)),
                        }
                    }else{Err(DriverError::Protocol("Question expired".into()))};
                    let _=response.send(result);
                }
                Some(SessionCommand::Approve {
                    native_request_id,
                    option_id,
                    response,
                }) => {
                    let Some(jsonrpc_id) = pending_approvals.remove(&native_request_id) else {
                        let _ = response.send(Err(DriverError::CapabilityUnavailable(
                            "this approval request is no longer pending".into(),
                        )));
                        continue;
                    };
                    // Only the two protocol decisions are answerable.
                    let decision = match option_id.as_str() {
                        "allow" => "accept",
                        "decline" => "decline",
                        _ => {
                            pending_approvals.insert(native_request_id, jsonrpc_id);
                            let _ = response.send(Err(DriverError::CapabilityUnavailable(
                                "unknown approval option id".into(),
                            )));
                            continue;
                        }
                    };
                    let result = connection
                        .transport
                        .send_result(jsonrpc_id, json!({ "decision": decision }))
                        .await;
                    let _ = response.send(result);
                }
                Some(SessionCommand::Interrupt { response }) => {
                    interrupts.0.push(response);
                    if stop_deadline.is_some() { continue; }
                    stop_deadline=Some(tokio::time::Instant::now()+stop_timeout);
                    let thread_id = connection.thread_id.clone();
                    let result = connection.request(
                        "turn/interrupt",
                        json!({ "threadId": thread_id, "turnId": started.native_turn_id }),
                    ).await;
                    match result {
                        Ok(exchange) => {
                            for frame in exchange.interleaved {
                                verified_terminal |= confirms_turn(&frame,&connection.thread_id,&started.native_turn_id);
                                terminal |= handle_frame(
                                    connection,
                                    events,
                                    frame,
                                    &mut pending_approvals,
                                    &mut next_approval_id,
                                    &mut file_changes,
                                )
                                .await;
                            }
                        }
                        Err(error) => {
                            interrupts.finish(Err(error));
                            connection.native.request_close();
                            return ActorControl::Stop;
                        }
                    }
                }
                Some(SessionCommand::Configure { response, .. }) => {
                    let _ = response.send(Err(DriverError::Protocol("cannot configure an active codex turn".into())));
                }
                Some(SessionCommand::CheckReady { response, .. }) => {
                    let _ = response.send(Err(DriverError::CapabilityUnavailable("codex turn is already active".into())));
                }
                Some(SessionCommand::Shutdown { response }) => {
                    return ActorControl::Shutdown(response);
                }
                Some(SessionCommand::Prompt { response, .. }) => {
                    let _ = response.send(Err(DriverError::CapabilityUnavailable(
                        "codex turn is already active".into(),
                    )));
                }
                None => return ActorControl::Stop,
            },
            frame = connection.transport.read_frame() => match frame {
                Ok(frame) => {
                    verified_terminal |= confirms_turn(&frame,&connection.thread_id,&started.native_turn_id);
                    terminal |= handle_frame(
                        connection,
                        events,
                        frame,
                        &mut pending_approvals,
                        &mut next_approval_id,
                        &mut file_changes,
                    )
                    .await;
                }
                Err(error) => {
                    let _ = events.send(Err(error)).await;
                    return ActorControl::Stop;
                }
            },
            ()=async { if let Some(deadline)=stop_deadline {tokio::time::sleep_until(deadline).await;} else {std::future::pending::<()>().await;} }=>{
                connection.native.request_close();
                interrupts.finish(Err(DriverError::Disconnected("Codex stop did not receive a terminal event in time".into())));
                let _=events.send(Err(DriverError::Disconnected("Codex stop was not confirmed; closing the original worker".into()))).await;
                return ActorControl::Stop;
            },
            ()=events.closed()=>{
                connection.native.request_close();
                return ActorControl::Stop;
            },
        }
    }
    if !interrupts.0.is_empty() {
        if !verified_terminal {
            return ActorControl::Stop;
        }
        interrupts.finish(Ok(()));
    }
    ActorControl::Continue
}

async fn handle_frame(
    connection: &mut Connection,
    events: &mpsc::Sender<Result<DriverEvent, DriverError>>,
    frame: IncomingFrame,
    pending_approvals: &mut HashMap<String, Value>,
    next_approval_id: &mut u64,
    file_changes: &mut file_approval::FileChanges,
) -> bool {
    match frame {
        IncomingFrame::Notification { method, params } => {
            let previous = connection
                .quota
                .as_ref()
                .and_then(|quota| quota.usage.get(Some(&connection.thread_id)));
            if let Some(request) =
                connection
                    .live_usage
                    .observe(&method, &params, previous.as_ref())
            {
                if let Some(quota) = &connection.quota {
                    quota
                        .usage
                        .observe_request(&connection.thread_id, request.clone());
                }
                let _ = events
                    .send(Ok(DriverEvent::UsageRecorded { request }))
                    .await;
            }
            if method == "serverRequest/resolved" {
                let ids: Vec<_> = pending_approvals
                    .iter()
                    .filter(|(_, v)| {
                        v.get("questionId") == params.get("requestId")
                            && v.get("questionId").is_some()
                    })
                    .map(|(k, _)| k.clone())
                    .collect();
                for id in ids {
                    pending_approvals.remove(&id);
                    let _ = events
                        .send(Ok(DriverEvent::QuestionClosed {
                            native_request_id: id,
                        }))
                        .await;
                }
            }
            file_changes.observe(&method, &params);
            for event in driver_api::presentation::presentation_events("codex", &method, &params) {
                let _ = events.send(Ok(event)).await;
            }
            if let Some(quota) = &connection.quota
                && matches!(
                    method.as_str(),
                    "account/rateLimits/updated" | "thread/tokenUsage/updated"
                )
            {
                quota.observe_notification(method.as_str(), &params);
                return false;
            }
            if !connection.live_usage.accepts(&params) {
                return false;
            }
            if let Some(event) = map_notification(&method, &params) {
                let terminal = matches!(
                    event,
                    DriverEvent::TurnCompleted { .. }
                        | DriverEvent::TurnFailed { .. }
                        | DriverEvent::TurnQuotaExhausted { .. }
                        | DriverEvent::TurnInterrupted { .. }
                );
                if terminal {
                    for request in connection.live_usage.finish(previous.and_then(|s| s.model)) {
                        if let Some(quota) = &connection.quota {
                            quota
                                .usage
                                .observe_request(&connection.thread_id, request.clone());
                        }
                        let _ = events
                            .send(Ok(DriverEvent::UsageRecorded { request }))
                            .await;
                    }
                }
                let _ = events.send(Ok(event)).await;
                terminal
            } else {
                false
            }
        }
        IncomingFrame::ServerRequest { id, method, params } => {
            if method == "item/tool/requestUserInput" {
                match driver_api::questions::native_form("codex", &params) {
                    Ok(form) => {
                        let request_id =
                            format!("codex-question-{}", agent_domain::InstallationId::new());
                        pending_approvals
                            .insert(request_id.clone(), json!({"questionId":id,"params":params}));
                        let _ = events
                            .send(Ok(DriverEvent::QuestionRequested {
                                native_request_id: request_id,
                                form,
                            }))
                            .await;
                    }
                    Err(_) => {
                        let _ = connection
                            .transport
                            .send_error(id, -32602, "Unsupported question form")
                            .await;
                    }
                }
                return false;
            }

            // Surface the vendor approval verbatim and park the jsonrpc id;
            // the hub answers through respond_to_interaction with one of the
            // two protocol decisions. No auto-decline path remains.
            let approval = match method.as_str() {
                "item/commandExecution/requestApproval" => Some((
                    "commandExecution",
                    params
                        .get("command")
                        .map(std::string::ToString::to_string)
                        .unwrap_or_default(),
                )),
                "item/fileChange/requestApproval" => {
                    Some(("fileChange", file_changes.summary(&params)))
                }
                _ => None,
            };
            let Some((tool_name, command_summary)) = approval else {
                let response = connection
                    .transport
                    .send_error(id, -32601, "unsupported client callback")
                    .await;
                if let Err(error) = response {
                    let _ = events.send(Err(error)).await;
                    return true;
                }
                return false;
            };
            let native_request_id = format!("codex-approval-{next_approval_id}");
            *next_approval_id += 1;
            pending_approvals.insert(native_request_id.clone(), id);
            let _ = events
                .send(Ok(DriverEvent::ApprovalRequested {
                    native_request_id,
                    tool_name: tool_name.into(),
                    command_summary,
                    options: vec![
                        driver_api::ApprovalOption {
                            option_id: "allow".into(),
                            label: "Allow once".into(),
                            semantic_role: driver_api::ApprovalSemanticRole::ApproveOnce,
                        },
                        driver_api::ApprovalOption {
                            option_id: "decline".into(),
                            label: "Decline".into(),
                            semantic_role: driver_api::ApprovalSemanticRole::Reject,
                        },
                    ],
                }))
                .await;
            false
        }
        IncomingFrame::Response => false,
    }
}

fn file_change_approval_summary(params: &serde_json::Value) -> String {
    // Some server versions omit file details on the approval request. Do not
    // turn that omission into a blank or apparently scoped approval.
    let files = params.get("files").filter(|value| match value {
        serde_json::Value::Array(files) => !files.is_empty(),
        serde_json::Value::Object(files) => !files.is_empty(),
        _ => false,
    });
    serde_json::json!({
        "scope": if files.is_some() { "reported_files" } else { "unknown" },
        "message": if files.is_some() { "文件修改审批" } else { "Agent 未提供具体文件修改范围；请勿在未核对修改内容时批准。" },
        "files": files,
        "itemId": params.get("itemId").and_then(serde_json::Value::as_str),
        "reason": params.get("reason").and_then(serde_json::Value::as_str),
        "grantRoot": params.get("grantRoot").and_then(serde_json::Value::as_str),
    }).to_string()
}

fn map_notification(method: &str, params: &Value) -> Option<DriverEvent> {
    match method {
        "item/agentMessage/delta" => Some(DriverEvent::TextDelta {
            item_id: params
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or("codex-message")
                .to_owned(),
            text: params
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }),
        "item/started" => map_tool_item(params, false),
        "item/completed" => map_tool_item(params, true),
        "item/commandExecution/started" => Some(DriverEvent::ToolStarted {
            item_id: notification_item_id(params),
            tool_name: "command_execution".into(),
            summary: "运行命令".into(),
        }),
        "item/commandExecution/completed" => Some(DriverEvent::ToolCompleted {
            item_id: notification_item_id(params),
            tool_name: "command_execution".into(),
            summary: "运行命令".into(),
            success: item_completed_success(params.get("item").unwrap_or(params)),
        }),
        "turn/diff/updated" => Some(DriverEvent::ProtocolWarning {
            message: "Codex emitted a diff update; workspace snapshots remain authoritative".into(),
        }),
        "turn/completed" => {
            let status = params
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str);
            Some(match status {
                Some("completed") => DriverEvent::TurnCompleted {
                    summary: "Codex turn completed".into(),
                },
                Some("interrupted") => DriverEvent::TurnInterrupted {
                    summary: "Codex turn interrupted".into(),
                },
                Some("failed") => codex_failed_turn(params),
                _ => DriverEvent::TurnFailed {
                    reason: "Codex turn ended with an unknown status".into(),
                },
            })
        }
        // turn/start's reviewed response already produced exactly one TurnStarted event.
        // mcpServer/thread status churn is progress noise.
        "turn/started"
        | "thread/started"
        | "thread/status/changed"
        | "mcpServer/startupStatus/updated" => None,
        "configWarning" | "warning" | "error" => Some(DriverEvent::ProtocolWarning {
            message: "Codex reported a runtime warning; raw text was omitted".into(),
        }),
        _ => Some(DriverEvent::ProtocolWarning {
            message: "Codex emitted an unrecognized event; raw payload was omitted".into(),
        }),
    }
}

fn codex_failed_turn(params: &Value) -> DriverEvent {
    let turn = params.get("turn").unwrap_or(params);
    let error = turn.get("error").unwrap_or(&Value::Null);
    match request_failure::map(error) {
        DriverError::QuotaExhausted { resets_at_unix } => {
            DriverEvent::TurnQuotaExhausted { resets_at_unix }
        }
        failure => {
            let kind = driver_api::failure::kind(&failure);
            let message = [error, &error["data"], &error["data"]["error"]]
                .into_iter()
                .find_map(|value| value["message"].as_str().or_else(|| value.as_str()));
            DriverEvent::TurnFailed {
                reason: message.map_or_else(
                    || driver_api::failure::turn_failed_reason(&failure),
                    |message| driver_api::failure::with_detail(kind, message),
                ),
            }
        }
    }
}

fn map_tool_item(params: &Value, completed: bool) -> Option<DriverEvent> {
    let item = params.get("item")?;
    let item_type = item.get("type").and_then(Value::as_str)?;
    let (tool_name, summary) = match item_type {
        "commandExecution" => ("command_execution".into(), "运行命令".into()),
        "fileChange" => ("file_change".into(), "修改文件".into()),
        "mcpToolCall" => {
            let name = item
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("MCP 工具");
            (name.to_owned(), format!("调用 {name}"))
        }
        "dynamicToolCall" => {
            let name = item
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("动态工具");
            (name.to_owned(), format!("调用 {name}"))
        }
        "collabAgentToolCall" => ("collab_agent".into(), "协调子 Agent".into()),
        "subAgentActivity" => ("sub_agent".into(), "执行子 Agent 任务".into()),
        "webSearch" => ("web_search".into(), "搜索网页".into()),
        "imageView" => ("image_view".into(), "查看图片".into()),
        "sleep" => ("sleep".into(), "等待任务".into()),
        "imageGeneration" => ("image_generation".into(), "生成图片".into()),
        _ => return None,
    };
    let item_id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("codex-tool")
        .to_owned();
    Some(if completed {
        DriverEvent::ToolCompleted {
            item_id,
            tool_name,
            summary,
            success: item_completed_success(item),
        }
    } else {
        DriverEvent::ToolStarted {
            item_id,
            tool_name,
            summary,
        }
    })
}

fn notification_item_id(params: &Value) -> String {
    params
        .get("item")
        .and_then(|item| item.get("id"))
        .or_else(|| params.get("itemId"))
        .and_then(Value::as_str)
        .unwrap_or("codex-command")
        .to_owned()
}

fn item_completed_success(item: &Value) -> bool {
    if let Some(success) = item.get("success").and_then(Value::as_bool) {
        return success;
    }
    if item.get("error").is_some_and(|value| !value.is_null())
        || item.get("failure").is_some_and(|value| !value.is_null())
    {
        return false;
    }
    item.get("status")
        .and_then(Value::as_str)
        .is_none_or(|status| matches!(status, "completed" | "success" | "succeeded"))
}

struct Connection {
    live_usage: live_usage::LiveUsage,
    analysis_only: bool,
    full_review: bool,
    native: Arc<lifecycle::NativeProcess>,
    quota: Option<Arc<QuotaState>>,
    transport: JsonlTransport<tokio::io::DuplexStream, ChildStdin>,
    thread_id: String,
    selection: Option<(String, Option<String>)>,
    #[cfg(test)]
    stop_timeout: Duration,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.native.request_close();
    }
}

impl Connection {
    async fn model_catalog(&mut self) -> Result<driver_api::ModelCatalog, DriverError> {
        let mut catalog = driver_api::ModelCatalog::default();
        let mut cursor: Option<String> = None;
        for _ in 0..20 {
            let response = self
                .request("model/list", json!({"cursor": cursor, "limit": 100}))
                .await?;
            let page = driver_api::models::codex_page(&response.result);
            if page.default_model.is_some() {
                catalog.default_model = page.default_model;
            }
            for model in page.models {
                if !catalog.models.iter().any(|m| m.id == model.id) {
                    catalog.models.push(model);
                }
            }
            let next = response.result["nextCursor"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            if next.is_none() {
                return Ok(catalog);
            }
            if next == cursor {
                return Err(stable_error("model catalog cursor repeated"));
            }
            cursor = next;
        }
        Err(stable_error("model catalog pagination exceeded limit"))
    }
    async fn configure_selection(
        &mut self,
        model: Option<String>,
        thinking: Option<String>,
    ) -> Result<(), DriverError> {
        let selection = if let (Some(model), Some(thinking)) = (&model, &thinking) {
            (model.clone(), Some(thinking.clone()))
        } else {
            let catalog = self.model_catalog().await?;
            let model = model
                .or(catalog.default_model)
                .ok_or_else(|| stable_error("codex default model unavailable"))?;
            let option = catalog
                .models
                .iter()
                .find(|option| option.id == model)
                .ok_or_else(|| stable_error("codex selected model unavailable"))?;
            let thinking = thinking.or(option.default_thinking.clone());
            if thinking.is_none() && !option.thinking_options.is_empty() {
                return Err(stable_error("codex default effort unavailable"));
            }
            (model, thinking)
        };
        self.selection = Some(selection);
        Ok(())
    }

    async fn spawn(executable: &Path) -> Result<Self, DriverError> {
        let mut connection = Self::create(executable)?;
        connection.initialize().await?;
        Ok(connection)
    }

    fn create(executable: &Path) -> Result<Self, DriverError> {
        let mut command = Command::new(executable);
        command
            .args(["app-server", "--listen", "stdio://"])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        agent_runtime::apply_platform_environment(&mut command);
        for name in [
            "PATH",
            "HOME",
            "TMPDIR",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
        ] {
            if let Some(value) = agent_runtime::inherited_variable(name) {
                command.env(name, value);
            }
        }
        agent_runtime::apply_proxy_environment(&mut command);
        let child = agent_runtime::OwnedChild::spawn(command)
            .map_err(|_| DriverError::Disconnected("codex app-server could not start".into()))?;
        let (native, stdout, stdin) = lifecycle::NativeProcess::start(child)?;
        let mut transport = JsonlTransport::new(stdout, stdin);
        transport.native = Some(native.clone());
        Ok(Self {
            live_usage: live_usage::LiveUsage::default(),
            analysis_only: false,
            full_review: false,
            native,
            quota: None,
            transport,
            thread_id: String::new(),
            selection: None,
            #[cfg(test)]
            stop_timeout: REQUEST_TIMEOUT,
        })
    }

    async fn initialize(&mut self) -> Result<(), DriverError> {
        let exchange = self
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "dogcodes",
                        "title": "dogcodes",
                        "version": DRIVER_VERSION
                    },
                    "capabilities": { "experimentalApi": true }
                }),
            )
            .await;
        let exchange = match exchange {
            Ok(exchange) => exchange,
            Err(error) => {
                self.shutdown().await?;
                return Err(error);
            }
        };
        if !exchange.result.is_object() {
            self.shutdown().await?;
            return Err(stable_error("codex initialize response is invalid"));
        }
        let initialized = self.transport.send_notification("initialized", None).await;
        if let Err(error) = initialized {
            self.shutdown().await?;
            return Err(error);
        }
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Exchange, DriverError> {
        timeout(REQUEST_TIMEOUT, self.transport.request(method, params))
            .await
            .map_err(|_| DriverError::Disconnected("codex request timed out".into()))?
    }

    async fn shutdown(&mut self) -> Result<(), DriverError> {
        self.transport.writer.take();
        self.native.request_close();
        // A bounded waiter may fail, but registry/native ownership survives it.
        // Only the independent supervisor publishes actual exit confirmation.
        self.native.closed().await
    }
}

struct Exchange {
    result: Value,
    interleaved: Vec<IncomingFrame>,
}

enum IncomingFrame {
    Response,
    Notification {
        method: String,
        params: Value,
    },
    ServerRequest {
        id: Value,
        method: String,
        params: Value,
    },
}

struct JsonlTransport<R, W> {
    reader: BufReader<R>,
    read_buffer: Vec<u8>,
    writer: Option<W>,
    next_request_id: u64,
    broken: Arc<AtomicBool>,
    native: Option<Arc<lifecycle::NativeProcess>>,
}

struct IoFence {
    broken: Arc<AtomicBool>,
    native: Option<Arc<lifecycle::NativeProcess>>,
    complete: bool,
}
impl Drop for IoFence {
    fn drop(&mut self) {
        if !self.complete {
            self.broken.store(true, Ordering::Release);
            if let Some(native) = &self.native {
                native.request_close();
            }
        }
    }
}

impl<R, W> JsonlTransport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            read_buffer: Vec::with_capacity(8 * 1024),
            writer: Some(writer),
            next_request_id: 1,
            broken: Arc::new(AtomicBool::new(false)),
            native: None,
        }
    }

    fn fence(&self) -> IoFence {
        IoFence {
            broken: self.broken.clone(),
            native: self.native.clone(),
            complete: false,
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Exchange, DriverError> {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.send_value(&json!({ "id": id, "method": method, "params": params }))
            .await?;
        let mut pending = self.fence();
        let mut interleaved = Vec::new();
        loop {
            let raw = self.read_value().await?;
            if raw.get("id") == Some(&Value::from(id))
                && (raw.get("result").is_some() || raw.get("error").is_some())
            {
                pending.complete = true;
                if let Some(error) = raw.get("error") {
                    return Err(request_failure::map(error));
                }
                return Ok(Exchange {
                    result: raw.get("result").cloned().unwrap_or(Value::Null),
                    interleaved,
                });
            }
            if interleaved.len() == MAX_INTERLEAVED_FRAMES {
                return Err(stable_error("codex interleaved frame budget exceeded"));
            }
            interleaved.push(classify_frame(raw)?);
        }
    }

    async fn read_frame(&mut self) -> Result<IncomingFrame, DriverError> {
        let result = self.read_value().await.and_then(classify_frame);
        if result.is_err() {
            drop(self.fence());
        }
        result
    }

    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), DriverError> {
        let mut value = json!({ "method": method });
        if let Some(params) = params {
            value["params"] = params;
        }
        self.send_value(&value).await
    }

    async fn send_result(&mut self, id: Value, result: Value) -> Result<(), DriverError> {
        self.send_value(&json!({ "id": id, "result": result }))
            .await
    }

    async fn send_error(&mut self, id: Value, code: i64, message: &str) -> Result<(), DriverError> {
        self.send_value(&json!({ "id": id, "error": { "code": code, "message": message } }))
            .await
    }

    async fn send_value(&mut self, value: &Value) -> Result<(), DriverError> {
        if self.broken.load(Ordering::Acquire)
            || self.native.as_ref().is_some_and(|native| native.stopping())
        {
            return Err(DriverError::Disconnected(
                "Codex transport is closing".into(),
            ));
        }
        let bytes =
            serde_json::to_vec(value).map_err(|_| stable_error("cannot encode Codex frame"))?;
        if bytes.len() > MAX_RPC_FRAME_BYTES {
            return Err(stable_error(
                "outbound Codex frame exceeds configured limit",
            ));
        }
        let mut frame = self.fence();
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| stable_error("Codex stdin is closed"))?;
        timeout(REQUEST_TIMEOUT, async {
            writer.write_all(&bytes).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await
        })
        .await
        .map_err(|_| DriverError::Disconnected("Codex transport write timed out".into()))?
        .map_err(|_| DriverError::Disconnected("codex transport write failed".into()))?;
        frame.complete = true;
        Ok(())
    }

    async fn read_value(&mut self) -> Result<Value, DriverError> {
        let result = self.read_value_inner().await;
        if result.is_err() {
            drop(self.fence());
        }
        result
    }

    async fn read_value_inner(&mut self) -> Result<Value, DriverError> {
        if self.broken.load(Ordering::Acquire) {
            return Err(DriverError::Disconnected(
                "Codex transport is closing".into(),
            ));
        }
        loop {
            let available = self
                .reader
                .fill_buf()
                .await
                .map_err(|_| DriverError::Disconnected("codex transport read failed".into()))?;
            if available.is_empty() {
                return Err(DriverError::Disconnected("codex transport closed".into()));
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.unwrap_or(available.len());
            if self.read_buffer.len().saturating_add(count) > MAX_RPC_FRAME_BYTES {
                return Err(stable_error("inbound Codex frame exceeds configured limit"));
            }
            self.read_buffer.extend_from_slice(&available[..count]);
            let consumed = count + usize::from(newline.is_some());
            self.reader.consume(consumed);
            if newline.is_some() {
                let bytes = std::mem::take(&mut self.read_buffer);
                return serde_json::from_slice(&bytes)
                    .map_err(|_| stable_error("Codex emitted invalid JSON"));
            }
        }
    }
}

fn classify_frame(raw: Value) -> Result<IncomingFrame, DriverError> {
    let object = raw
        .as_object()
        .ok_or_else(|| stable_error("Codex emitted an invalid frame shape"))?;
    let id = object.get("id").cloned();
    let method = object.get("method").and_then(Value::as_str);
    match (id, method) {
        (None, Some(method)) => Ok(IncomingFrame::Notification {
            method: method.into(),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        }),
        (Some(id), Some(method)) if id.is_string() || id.is_number() => {
            Ok(IncomingFrame::ServerRequest {
                id,
                method: method.into(),
                params: object.get("params").cloned().unwrap_or_else(|| json!({})),
            })
        }
        (Some(_), None) if object.contains_key("result") || object.contains_key("error") => {
            Ok(IncomingFrame::Response)
        }
        _ => Err(stable_error("Codex emitted an invalid frame shape")),
    }
}

async fn drain_stderr<R>(mut stderr: R)
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

fn stable_error(message: &str) -> DriverError {
    DriverError::Protocol(message.into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stop_is_confirmed_by_this_turn_or_an_unlabelled_terminal_event() {
        let frame = |params: serde_json::Value| super::IncomingFrame::Notification {
            method: "turn/completed".into(),
            params,
        };
        let confirms = |params| super::confirms_turn(&frame(params), "thread", "turn");
        assert!(confirms(
            serde_json::json!({"threadId":"thread","turn":{"id":"turn","status":"interrupted"}})
        ));
        assert!(confirms(
            serde_json::json!({"turn":{"status":"interrupted"}})
        ));
        assert!(!confirms(
            serde_json::json!({"threadId":"other","turn":{"id":"turn","status":"interrupted"}})
        ));
        assert!(!confirms(
            serde_json::json!({"threadId":"thread","turn":{"id":"older","status":"completed"}})
        ));
        assert!(!confirms(
            serde_json::json!({"threadId":"thread","turn":{"id":"turn","status":"inProgress"}})
        ));
    }
    #[test]
    fn missing_file_approval_scope_is_explicit_and_does_not_copy_extensions() {
        let summary = super::file_change_approval_summary(
            &serde_json::json!({"itemId":"edit-1", "grantRoot":"/temporary", "reason":"edit requested", "privateExtension":"must not display"}),
        );
        let value: serde_json::Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(value["scope"], "unknown");
        assert_eq!(value["grantRoot"], "/temporary");
        assert!(!summary.contains("must not display"));
        assert!(value["message"].as_str().unwrap().contains("未提供"));
        for files in [
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::Value::Null,
        ] {
            let summary = super::file_change_approval_summary(&serde_json::json!({"files":files}));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&summary).unwrap()["scope"],
                "unknown"
            );
        }
        let summary = super::file_change_approval_summary(
            &serde_json::json!({"files":[{"path":"sample.txt"}]}),
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&summary).unwrap()["scope"],
            "reported_files"
        );
    }
    use super::*;

    #[test]
    fn parses_account_rate_limits_and_prefers_shortest_window() {
        let parsed = parse_rate_limits(Some(&json!({
            "rateLimits": {
                "limitId": "codex",
                "planType": "plus",
                "primary": { "usedPercent": 39, "windowDurationMins": 300, "resetsAt": 1_800 },
                "secondary": { "usedPercent": 63, "windowDurationMins": 10_080, "resetsAt": 2_400 }
            },
            "rateLimitsByLimitId": {
                "codex_spark": {
                    "primary": { "usedPercent": 99, "windowDurationMins": 60 }
                }
            }
        })))
        .unwrap();
        assert_eq!(parsed.plan_type.as_deref(), Some("plus"));
        assert_eq!(parsed.windows.len(), 2);
        assert_eq!(parsed.windows[0].window_minutes, 300);
        assert_eq!(parsed.windows[0].used_percent, Some(39.0));
        assert_eq!(parsed.windows[1].window_minutes, 10_080);
    }

    #[test]
    fn maps_only_explicit_codex_quota_failures_to_structured_terminal() {
        assert!(matches!(
            codex_failed_turn(&json!({
                "turn": { "error": { "code": "usage_limit_reached", "resetsAt": 1_800_000_000 } }
            })),
            DriverEvent::TurnQuotaExhausted {
                resets_at_unix: Some(1_800_000_000)
            }
        ));
        assert!(matches!(
            codex_failed_turn(&json!({ "turn": { "error": { "message": "HTTP 429" } } })),
            DriverEvent::TurnFailed { .. }
        ));
    }

    #[test]
    fn account_rejection_discards_cached_quota_but_preserves_session_context() {
        let state = QuotaState::default();
        state.record(
            QuotaSnapshot {
                collected_at_unix_ms: 1,
                ..Default::default()
            },
            "native_pull",
        );
        state.context_by_thread.lock().unwrap().insert(
            "thread-a".into(),
            ContextUsageSnapshot {
                used_tokens: 10,
                window_tokens: 100,
                cache_hit_rate_percent: None,
            },
        );
        let cached = state.snapshot_for(None);
        assert!(matches!(
            state.quota_failure(
                cached,
                DriverError::NotConfigured("authentication_failed".into())
            ),
            Err(DriverError::NotConfigured(_))
        ));
        assert!(state.snapshot_for(None).is_none());
        assert!(
            state
                .context_by_thread
                .lock()
                .unwrap()
                .contains_key("thread-a")
        );
    }

    #[test]
    fn rejects_model_specific_rate_limit_pushes() {
        assert!(
            parse_rate_limits(Some(&json!({
                "limitId": "codex_spark",
                "primary": { "usedPercent": 2, "windowDurationMins": 300 }
            })))
            .is_none()
        );
    }

    #[test]
    fn scopes_context_usage_to_the_requested_thread() {
        let state = QuotaState::default();
        state.observe_notification(
            "thread/tokenUsage/updated",
            &json!({
                "threadId": "thread-a",
                "turnId": "turn-a",
                "tokenUsage": {
                    "last": { "totalTokens": 240 },
                    "total": { "inputTokens": 800, "cachedInputTokens": 600 },
                    "modelContextWindow": 2_000
                }
            }),
        );
        let collected_at_unix_ms = now_unix_ms_i64();
        state.record(
            QuotaSnapshot {
                balances: Vec::new(),
                session_usage: None,
                windows: vec![QuotaWindow {
                    scope: None,
                    id: "primary".into(),
                    window_minutes: 300,
                    used_percent: Some(10.0),
                    resets_at_unix: None,
                }],
                context: None,
                collected_at_unix_ms,
                stale_after_unix_ms: collected_at_unix_ms + 15_000,
                plan_type: None,
                source: "native_pull".into(),
                confidence: "provider_reported".into(),
                parser_version: QUOTA_PARSER_VERSION.into(),
            },
            "native_pull",
        );

        let thread_a = state.snapshot_for(Some("thread-a")).unwrap();
        assert_eq!(thread_a.context.unwrap().used_tokens, 240);
        assert!(
            state
                .snapshot_for(Some("thread-b"))
                .unwrap()
                .context
                .is_none()
        );
        assert!(state.snapshot_for(None).unwrap().context.is_none());

        state.observe_notification(
            "account/rateLimits/updated",
            &json!({
                "rateLimits": {
                    "limitId": "codex",
                    "primary": { "usedPercent": 11, "windowDurationMins": 300 }
                }
            }),
        );
        let invalidated = state.snapshot_for(None).unwrap();
        assert!(invalidated.stale_after_unix_ms <= now_unix_ms_i64());
        assert_eq!(invalidated.windows[0].used_percent, Some(10.0));
    }

    #[test]
    fn maps_only_reviewed_notification_fields() {
        let raw =
            json!({"turn":{"status":"completed","error":{"message":"SECRET"}},"token":"SECRET"});
        let event = map_notification("turn/completed", &raw).unwrap();
        assert_eq!(
            event,
            DriverEvent::TurnCompleted {
                summary: "Codex turn completed".into()
            }
        );
        assert!(!serde_json::to_string(&event).unwrap().contains("SECRET"));

        let unknown =
            map_notification("SECRET_METHOD", &json!({"password":"SECRET_PAYLOAD"})).unwrap();
        let serialized = serde_json::to_string(&unknown).unwrap();
        assert!(!serialized.contains("SECRET_METHOD"));
        assert!(!serialized.contains("SECRET_PAYLOAD"));

        assert_eq!(
            map_notification(
                "item/started",
                &json!({"item":{"id":"web-1","type":"webSearch","query":"private query"}}),
            ),
            Some(DriverEvent::ToolStarted {
                item_id: "web-1".into(),
                tool_name: "web_search".into(),
                summary: "搜索网页".into(),
            })
        );
        assert_eq!(
            map_notification(
                "item/completed",
                &json!({"item":{"id":"web-1","type":"webSearch","query":"private query"}}),
            ),
            Some(DriverEvent::ToolCompleted {
                item_id: "web-1".into(),
                tool_name: "web_search".into(),
                summary: "搜索网页".into(),
                success: true,
            })
        );
    }

    #[tokio::test]
    async fn transport_rejects_oversized_frames_before_writing_or_parsing() {
        let mut transport = JsonlTransport::new(tokio::io::empty(), Vec::new());
        let result = transport
            .send_value(&json!({"text": "x".repeat(MAX_RPC_FRAME_BYTES)}))
            .await;
        assert!(matches!(result, Err(DriverError::Protocol(_))));
        assert!(transport.writer.as_ref().unwrap().is_empty());

        let oversized = vec![b'x'; MAX_RPC_FRAME_BYTES + 1];
        let mut transport = JsonlTransport::new(oversized.as_slice(), tokio::io::sink());
        assert!(
            matches!(transport.read_value().await, Err(DriverError::Protocol(message))
            if message == "inbound Codex frame exceeds configured limit")
        );
    }

    #[tokio::test]
    async fn request_rejections_preserve_account_categories_and_reset_time() {
        for (error, expected) in [
            (
                json!({"code":-32000,"data":{"codexErrorInfo":"usageLimitExceeded","reset_at":1_800_000_000},"message":"private-key"}),
                "quota_exhausted",
            ),
            (
                json!({"code":-32000,"message":"subscription required private-key"}),
                "subscription_required",
            ),
            (
                json!({"code":-32000,"data":{"codexErrorInfo":"usageLimitExceeded"},"message":"To use Codex with your ChatGPT plan, upgrade to Plus."}),
                "subscription_required",
            ),
            (
                json!({"code":-32000,"data":{"codexErrorInfo":"unauthorized"},"message":"private-key"}),
                "authentication_failed",
            ),
            (
                json!({"code":-32000,"data":{"codexErrorInfo":"rateLimitExceeded"},"message":"private-key"}),
                "agent_rate_limited",
            ),
            (
                json!({"code":-32000,"message":"HTTP 429 private-key"}),
                "driver_protocol_error",
            ),
        ] {
            let mut response = serde_json::to_vec(&json!({"id":1,"error":error})).unwrap();
            response.push(b'\n');
            let mut transport = JsonlTransport::new(response.as_slice(), tokio::io::sink());
            let error = transport
                .request("turn/start", json!({}))
                .await
                .err()
                .unwrap();
            assert_eq!(driver_api::failure::kind(&error), expected);
            assert!(!error.to_string().contains("private-key"));
            if expected == "quota_exhausted" {
                assert_eq!(
                    error,
                    DriverError::QuotaExhausted {
                        resets_at_unix: Some(1_800_000_000)
                    }
                );
            }
        }
    }

    #[tokio::test]
    async fn bounded_transport_preserves_interleaving() {
        let (client, server) = tokio::io::duplex(4096);
        let (client_read, client_write) = tokio::io::split(client);
        let (server_read, mut server_write) = tokio::io::split(server);
        let server = tokio::spawn(async move {
            let mut request = String::new();
            BufReader::new(server_read)
                .read_line(&mut request)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&request).unwrap()["method"],
                "initialize"
            );
            server_write.write_all(b"{\"method\":\"warning\",\"params\":{\"message\":\"SECRET\"}}\n{\"id\":1,\"result\":{\"ok\":true}}\n").await.unwrap();
        });
        let mut transport = JsonlTransport::new(client_read, client_write);
        let exchange = transport.request("initialize", json!({})).await.unwrap();
        assert_eq!(exchange.result["ok"], true);
        assert_eq!(exchange.interleaved.len(), 1);
        server.await.unwrap();
    }

    /// Real-turn acceptance: consumes real model quota and requires the
    /// operator's explicit authorization. Fixed no-tool prompts only.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "consumes real model quota; run only with explicit operator authorization"]
    async fn real_turn_acceptance_streams_reuses_and_interrupts() {
        use std::path::PathBuf;

        use futures::StreamExt;

        async fn collect_turn(
            driver: &CodexDriver,
            session: &driver_api::SessionHandle,
            prompt: &str,
        ) -> Vec<DriverEvent> {
            let stream = driver
                .prompt(
                    session,
                    PromptRequest {
                        attachments: vec![],
                        permission_mode: None,
                        additional_directories: vec![],
                        prompt: prompt.to_owned(),
                        working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
                        context: vec![],
                    },
                )
                .await
                .expect("turn must be accepted");
            tokio::time::timeout(
                std::time::Duration::from_mins(3),
                stream.map(|event| event.unwrap()).collect::<Vec<_>>(),
            )
            .await
            .expect("turn must finish in time")
        }

        let codex = PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex");
        CodexDriver::probe_ready(&codex)
            .await
            .expect("codex must pass the readiness probe");
        let driver = CodexDriver::new(
            &codex,
            agent_domain::InstallationId::new().to_string(),
            "0.153.0-alpha.5".into(),
        )
        .expect("driver construction must succeed");
        let session = driver
            .create_session(SessionRequest {
                history_mcp: None,
                permission_mode: None,
                additional_directories: vec![],
                working_directory: std::env::temp_dir().to_string_lossy().into_owned(),
            })
            .await
            .expect("session must be created");
        let first = collect_turn(&driver, &session, "Reply with exactly: OK").await;
        assert!(
            first.iter().any(|event| matches!(
                event,
                DriverEvent::TextDelta { text, .. } if !text.is_empty()
            )),
            "turn 1 must stream text: {first:?}"
        );
        assert!(
            first
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "turn 1 must complete: {first:?}"
        );
        println!("turn 1 (stream + complete): ok");

        let second = collect_turn(&driver, &session, "Reply with exactly: DONE").await;
        assert!(
            second
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
            "turn 2 must complete on the same live thread"
        );
        println!("turn 2 (live thread reuse): ok");

        let stream = driver
            .prompt(
                &session,
                PromptRequest {
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
                std::time::Duration::from_mins(3),
                stream.map(|event| event.unwrap()).collect::<Vec<_>>(),
            )
            .await
            .expect("turn 3 must finish in time")
        });
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        driver
            .interrupt(&session)
            .await
            .expect("interrupt must send");
        let third = collector.await.unwrap();
        assert!(
            third
                .iter()
                .any(|event| matches!(event, DriverEvent::TurnInterrupted { .. })),
            "turn 3 must end interrupted: {third:?}"
        );
        println!("turn 3 (interrupt): ok");
    }
}

/// Cross-process resume acceptance: thread/resume on a fresh driver
/// instance must reattach and continue the conversation.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "consumes real model quota; run only with explicit operator authorization"]
async fn real_resume_acceptance_reattaches_across_instances() {
    use driver_api::SessionRequest;
    use futures::StreamExt;
    use std::path::PathBuf;

    let codex = PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex");
    let detected = tokio::process::Command::new(&codex)
        .arg("--version")
        .output()
        .await
        .unwrap();
    assert!(detected.status.success());
    let native_version = String::from_utf8(detected.stdout)
        .unwrap()
        .trim()
        .strip_prefix("codex-cli ")
        .expect("Codex version signature")
        .to_owned();
    let workspace = tempfile::tempdir().unwrap();
    let first = CodexDriver::new(
        &codex,
        agent_domain::InstallationId::new().to_string(),
        native_version.clone(),
    )
    .unwrap();
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
    let events: Vec<DriverEvent> = tokio::time::timeout(
        std::time::Duration::from_mins(3),
        first
            .prompt(
                &session,
                PromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "Remember the word: dogcodes. Reply with exactly: remembered".into(),
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
            .any(|event| matches!(event, DriverEvent::TurnCompleted { .. })),
        "turn 1 events: {events:?}"
    );
    first.shutdown(session).await.unwrap();

    let second = CodexDriver::new(
        &codex,
        agent_domain::InstallationId::new().to_string(),
        native_version,
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
                PromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: "What word did I ask you to remember? Reply with exactly that word."
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
    let text = events
        .iter()
        .filter_map(|event| match event {
            DriverEvent::TextDelta { text, .. } => Some(text.to_ascii_lowercase()),
            _ => None,
        })
        .collect::<String>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DriverEvent::TurnCompleted { .. }))
    );
    assert!(
        text.contains("dogcodes"),
        "resume must restore context: {text}"
    );
    println!("real resume (thread/resume reattach + recall): ok");
}

#[cfg(test)]
mod model_catalog_acceptance {
    use super::*;
    #[tokio::test]
    #[ignore = "reads the locally installed Codex model catalog, no model turn"]
    async fn local_model_catalog() {
        let path = std::env::var("DOGCODES_CODEX_BINARY").unwrap();
        let driver = CodexDriver::new(
            Path::new(&path),
            agent_domain::InstallationId::new().to_string(),
            "probe".into(),
        )
        .unwrap();
        let catalog = driver.model_catalog().await.unwrap();
        assert!(!catalog.models.is_empty());
        println!("{}", serde_json::to_string(&catalog).unwrap());
    }
}

fn question_session_config(
    history: Option<&driver_api::HistoryMcp>,
    thinking: Option<&str>,
) -> Value {
    let mut config = driver_api::HistoryMcp::codex_config(history, thinking);
    if !config.is_object() {
        config = json!({});
    }
    config["features.default_mode_request_user_input"] = json!(true);
    config
}

#[cfg(test)]
mod manual_quota_refresh_tests {
    use super::*;

    #[tokio::test]
    async fn manual_refresh_bypasses_fresh_cache_and_does_not_claim_cached_success() {
        let binary = tempfile::NamedTempFile::new().unwrap();
        let driver = CodexDriver::new(
            binary.path(),
            agent_domain::InstallationId::new().to_string(),
            "test".into(),
        )
        .unwrap();
        let snapshot = QuotaSnapshot {
            balances: Vec::new(),
            session_usage: None,
            windows: vec![driver_api::QuotaWindow {
                id: "5h".into(),
                scope: None,
                window_minutes: 300,
                used_percent: Some(20.0),
                resets_at_unix: None,
            }],
            collected_at_unix_ms: now_unix_ms_i64(),
            stale_after_unix_ms: now_unix_ms_i64() + 60_000,
            ..QuotaSnapshot::default()
        };
        driver.quota.record(snapshot.clone(), "native_pull");
        assert_eq!(driver.read_quota(None).await.unwrap(), snapshot);
        // Remove the fixture so a real pull fails without launching a process.
        drop(binary);
        assert!(driver.read_quota_with_refresh(None, true).await.is_err());
        assert_eq!(driver.quota.snapshot_for(None), Some(snapshot));
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    fn driver() -> Arc<CodexDriver> {
        Arc::new(
            CodexDriver::new(
                &std::env::current_exe().unwrap(),
                "fixture".into(),
                "fixture".into(),
            )
            .unwrap(),
        )
    }
    fn resident() -> (
        ResidentSession,
        mpsc::Receiver<ScopedCommand>,
        SessionHandle,
    ) {
        let (commands, receiver) = mpsc::channel(4);
        let handle = SessionHandle::scoped("fixture-session".into());
        (
            ResidentSession {
                commands,
                native: None,
                scope: Arc::new(std::sync::Mutex::new(handle.clone())),
            },
            receiver,
            handle,
        )
    }
    #[tokio::test]
    async fn cancelled_shutdown_retains_the_original_actor_until_confirmed() {
        let driver = driver();
        let (resident, mut actor, handle) = resident();
        driver
            .sessions
            .lock()
            .await
            .insert(handle.native_session_id.clone(), resident);
        let first_driver = driver.clone();
        let original_handle = handle.clone();
        let first = tokio::spawn(async move { first_driver.shutdown(original_handle).await });
        let SessionCommand::Shutdown {
            response: original_ack,
        } = actor.recv().await.unwrap().command
        else {
            panic!("expected shutdown");
        };
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert!(
            driver
                .sessions
                .lock()
                .await
                .contains_key(&handle.native_session_id)
        );
        let retry_driver = driver.clone();
        let retry_handle = handle.clone();
        let retry = tokio::spawn(async move { retry_driver.shutdown(retry_handle).await });
        let SessionCommand::Shutdown {
            response: confirmed_ack,
        } = actor.recv().await.unwrap().command
        else {
            panic!("expected original actor retry");
        };
        assert!(!retry.is_finished());
        confirmed_ack.send(Ok(())).unwrap();
        retry.await.unwrap().unwrap();
        assert!(
            !driver
                .sessions
                .lock()
                .await
                .contains_key(&handle.native_session_id)
        );
        assert!(original_ack.send(Ok(())).is_err());
        // Only an actual confirmed, scoped receipt makes repeated close safe.
        driver.shutdown(handle.clone()).await.unwrap();
        assert!(
            driver
                .shutdown(SessionHandle::new(handle.native_session_id))
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn late_shutdown_confirmation_does_not_remove_a_replacement_actor() {
        let driver = driver();
        let (original, mut actor, handle) = resident();
        driver
            .sessions
            .lock()
            .await
            .insert(handle.native_session_id.clone(), original);
        let closing_driver = driver.clone();
        let original_handle = handle.clone();
        let closing = tokio::spawn(async move { closing_driver.shutdown(original_handle).await });
        let SessionCommand::Shutdown { response } = actor.recv().await.unwrap().command else {
            panic!("expected shutdown");
        };
        let (replacement, _replacement_actor, replacement_handle) = resident();
        let replacement_commands = replacement.commands.clone();
        driver
            .sessions
            .lock()
            .await
            .insert(handle.native_session_id.clone(), replacement);
        response.send(Ok(())).unwrap();
        closing.await.unwrap().unwrap();
        assert!(
            driver.sessions.lock().await[&handle.native_session_id]
                .commands
                .same_channel(&replacement_commands)
        );
        assert!(driver.shutdown(handle.clone()).await.is_err());
        assert!(driver.shutdown_proof(&handle).await.is_none());
        assert!(
            driver
                .current_session_handle(&handle.native_session_id)
                .await
                .unwrap()
                .same_scope(&replacement_handle)
        );
    }
}
