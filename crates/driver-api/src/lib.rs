//! Stable ports implemented by coding-agent adapters.

pub mod attachments;
pub mod failure;
pub mod models;
pub mod permissions;
pub mod presentation;

pub mod questions;
pub mod usage;

pub use models::{ModelCatalog, ModelOption, ThinkingOption};
pub use permissions::PermissionCatalog;

use std::{
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
};

use agent_domain::{ArtifactId, ExecutorRef, ProviderId};
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub type DriverEventStream =
    Pin<Box<dyn Stream<Item = Result<DriverEvent, DriverError>> + Send + 'static>>;

/// Passive proof captured from the original resident before shutdown starts.
/// Resolving means its owned resources retired, never just a failed/expired call.
pub type SessionShutdownProof = Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHandle {
    pub native_session_id: String,
    // Runtime authority is deliberately absent from persisted/remote data.
    #[serde(skip)]
    scope: Option<ulid::Ulid>,
}

impl SessionHandle {
    /// A native reference for adapters without resident execution scopes.
    #[must_use]
    pub fn new(native_session_id: String) -> Self {
        Self {
            native_session_id,
            scope: None,
        }
    }

    /// A new resident incarnation, even if the vendor reuses its native ID.
    #[must_use]
    pub fn scoped(native_session_id: String) -> Self {
        Self {
            native_session_id,
            scope: Some(ulid::Ulid::new()),
        }
    }

    #[must_use]
    pub fn same_scope(&self, other: &Self) -> bool {
        self.native_session_id == other.native_session_id
            && self.scope.is_some()
            && self.scope == other.scope
    }
}

/// App-owned stdio MCP endpoint. Never persisted in task/session projections.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMcp {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub directory: String,
}
impl HistoryMcp {
    #[must_use]
    pub fn server_config(&self) -> serde_json::Value {
        serde_json::json!({"command":self.command,"args":self.args})
    }
    #[must_use]
    pub fn native_servers(value: Option<&Self>) -> serde_json::Value {
        serde_json::json!(value.map(|v| serde_json::json!({"name":v.name,"command":v.command,"args":v.args,"env":[]})).into_iter().collect::<Vec<_>>())
    }
    #[must_use]
    pub fn codex_config(value: Option<&Self>, thinking: Option<&str>) -> serde_json::Value {
        let mut config = serde_json::json!({});
        if let Some(v) = value {
            config["mcp_servers"] = serde_json::json!({v.name.clone():v.server_config()});
        }
        if let Some(v) = thinking {
            config["model_reasoning_effort"] = serde_json::json!(v);
        }
        config
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    #[serde(default)]
    pub history_mcp: Option<HistoryMcp>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub additional_directories: Vec<String>,
    pub working_directory: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    #[serde(default)]
    pub attachments: Vec<agent_domain::PromptAttachment>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    pub additional_directories: Vec<String>,
    pub prompt: String,
    pub working_directory: String,
    /// Ordered, user-reviewed context delivered before `prompt` in the same native turn.
    pub context: Vec<ContextDelivery>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextDeliveryKind {
    Handoff,
    Reconciliation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextDelivery {
    /// Local audit artifact from which the model-facing content was rendered.
    pub artifact_id: ArtifactId,
    pub kind: ContextDeliveryKind,
    /// Hash of the transmitted content, which may differ from the audit artifact hash.
    pub payload_sha256: String,
    pub content: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionResumeCapability {
    Supported,
    Unsupported,
    Unknown,
}

/// Stable semantic role of a vendor approval option. Only roles verified for
/// the driver's own protocol version may differ from [`ApprovalSemanticRole::Other`];
/// option labels are never used to guess a role.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalSemanticRole {
    ApproveOnce,
    Reject,
    Cancel,
    ApprovePersistent,
    Other,
}

/// One vendor-supplied approval option, verbatim from the agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOption {
    pub option_id: String,
    pub label: String,
    pub semantic_role: ApprovalSemanticRole,
}

/// One vendor quota window (for example Codex's 5-hour / 7-day buckets).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    /// Model/billing scope. None denotes an account-wide hard limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Vendor window label from the payload shape (e.g. "primary"); UI
    /// labels come from `window_minutes`, never from this field.
    pub id: String,
    pub window_minutes: u64,
    /// Vendor-reported utilization. `None` when the vendor exposes the
    /// window/reset time but no usage figure (e.g. Grok free tier).
    pub used_percent: Option<f64>,
    pub resets_at_unix: Option<i64>,
}

/// Live context-window usage for the active session, when the agent reports it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsageSnapshot {
    pub used_tokens: u64,
    pub window_tokens: u64,
    pub cache_hit_rate_percent: Option<f64>,
}

/// A point-in-time quota reading. `windows` is sorted by `window_minutes`
/// ascending, so the first entry is the most restrictive window.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub windows: Vec<QuotaWindow>,
    pub context: Option<ContextUsageSnapshot>,
    #[serde(default)]
    pub session_usage: Option<usage::SessionUsageSnapshot>,
    /// Unix milliseconds when the hub recorded this snapshot.
    pub collected_at_unix_ms: i64,
    /// Unix milliseconds after which the UI must mark the reading stale.
    pub stale_after_unix_ms: i64,
    /// Vendor-reported plan type, when present. Never inferred from pricing.
    pub plan_type: Option<String>,
    /// Stable source tag such as `native_pull` or `native_push`.
    pub source: String,
    /// Stable confidence tag. Provider-reported data uses `provider_reported`.
    pub confidence: String,
    /// Parser contract version for diagnostics.
    pub parser_version: String,
}

/// Subscription-usage port, deliberately separate from [`AgentDriver`].
///
/// Reading quota is a control-plane operation: it must never submit a model
/// prompt. Native credential reuse is permitted only for an explicitly
/// authorized read-only adapter (currently Cursor and `ZCode`); credentials must
/// never enter events, snapshots, logs, or persistence.
#[async_trait]
pub trait QuotaProvider: Send + Sync {
    fn executor(&self) -> &ExecutorRef;

    /// `native_session_id` scopes live context usage to the selected task.
    /// Account quota remains installation-wide when no session is supplied.
    async fn read_quota(
        &self,
        native_session_id: Option<&str>,
    ) -> Result<QuotaSnapshot, DriverError>;

    /// Read-only telemetry for exactly the requested session; never submit a prompt.
    async fn read_session_usage(
        &self,
        _native_session_id: &str,
    ) -> Option<usage::SessionUsageSnapshot> {
        None
    }

    /// Manual refresh bypasses local quota TTLs, never native auth/concurrency guards.
    async fn read_quota_with_refresh(
        &self,
        native_session_id: Option<&str>,
        _force: bool,
    ) -> Result<QuotaSnapshot, DriverError> {
        self.read_quota(native_session_id).await
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DriverEvent {
    /// First nonempty native reasoning/text output; no private reasoning content.
    FirstOutput,
    /// A settled native model request, delivered on its owning turn's stream.
    UsageRecorded {
        request: usage::UsageRequest,
    },
    /// Allowlisted display metadata; thought deltas are ephemeral at the hub.
    Presentation {
        item_id: String,
        data: Value,
    },
    SessionReady {
        native_session_id: String,
    },
    TurnStarted {
        native_turn_id: Option<String>,
    },
    TextDelta {
        item_id: String,
        text: String,
    },
    ToolStarted {
        item_id: String,
        tool_name: String,
        summary: String,
    },
    ToolCompleted {
        item_id: String,
        tool_name: String,
        summary: String,
        success: bool,
    },
    /// The agent asks for a tool approval. The hub must persist this before
    /// displaying it and may only answer with one of the vendor `options`.
    ApprovalRequested {
        native_request_id: String,
        tool_name: String,
        command_summary: String,
        options: Vec<ApprovalOption>,
    },
    QuestionClosed {
        native_request_id: String,
    },
    ApprovalClosed {
        native_request_id: String,
    },
    QuestionRequested {
        native_request_id: String,
        form: agent_domain::QuestionForm,
    },
    TurnCompleted {
        summary: String,
    },
    TurnFailed {
        reason: String,
    },
    /// The vendor explicitly reported that the account/model usage allowance
    /// is exhausted. This is distinct from transient throttling; drivers must
    /// only emit it for a verified code or an unambiguous vendor message.
    TurnQuotaExhausted {
        resets_at_unix: Option<i64>,
    },
    TurnInterrupted {
        summary: String,
    },
    VendorExtension {
        method: String,
        payload: Value,
    },
    ProtocolWarning {
        message: String,
    },
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DriverError {
    #[error("native session is busy with an autonomous continuation")]
    AutonomousTurnBusy,
    #[error("agent is not configured: {0}")]
    NotConfigured(String),
    #[error("agent disconnected: {0}")]
    Disconnected(String),
    #[error("driver protocol error: {0}")]
    Protocol(String),
    #[error("driver capability is unavailable: {0}")]
    CapabilityUnavailable(String),
    #[error("native session is missing: {0}")]
    SessionMissing(String),
    #[error("native session is incompatible: {0}")]
    SessionIncompatible(String),
    #[error("agent quota is exhausted")]
    QuotaExhausted { resets_at_unix: Option<i64> },
}

/// Conservative in-memory classifier for explicit vendor quota errors. Raw
/// vendor text must still be discarded before Canonical persistence. Generic
/// HTTP 429/rate-limit wording is intentionally excluded because it may only
/// mean transient request throttling.
#[must_use]
pub fn explicitly_reports_quota_exhaustion(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "usage limit reached",
        "usage limit has been reached",
        "usage limit exceeded",
        "usage-exhausted",
        "included free usage",
        "quota exceeded",
        "quota exhausted",
        "insufficient credits",
        "credit balance exhausted",
        "out of credits",
        "额度已达上限",
        "额度用尽",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

#[async_trait]
pub trait AgentDriver: Send + Sync {
    /// Whether the native transport accepts app-provided stdio MCP servers.
    /// Context handoff in the prompt remains available independently of MCP.
    fn supports_history_mcp(&self) -> bool {
        true
    }
    /// Optional native hardening; this is not an Agent availability gate.
    fn supports_readonly_analysis(&self) -> bool {
        false
    }
    /// Drivers sharing a connection/turn sink return a fresh adapter using the
    /// same installation. Multi-session drivers can reuse their existing adapter.
    fn independent_session_driver(
        &self,
    ) -> Result<Option<std::sync::Arc<dyn AgentDriver>>, DriverError> {
        Ok(None)
    }
    /// Some protocols reapply permissions on every prompt instead of session creation.
    fn analysis_prompt_permission_mode(&self) -> Option<&str> {
        None
    }
    async fn create_analysis_session(&self, directory: &str) -> Result<SessionHandle, DriverError> {
        let catalog = self.permission_catalog(Some(directory)).await?;
        let permission_mode = catalog.analysis_mode()?;
        self.create_session(SessionRequest {
            working_directory: directory.into(),
            additional_directories: vec![],
            history_mcp: None,
            permission_mode,
        })
        .await
    }

    /// Analysis sessions need the same selected project directories as normal sessions.
    async fn create_analysis_session_with_directories(
        &self,
        directory: &str,
        additional: &[String],
    ) -> Result<SessionHandle, DriverError> {
        if additional.is_empty() {
            return self.create_analysis_session(directory).await;
        }
        let permission_mode = self
            .permission_catalog(Some(directory))
            .await?
            .analysis_mode()?;
        self.create_session(SessionRequest {
            working_directory: directory.into(),
            additional_directories: additional.to_vec(),
            history_mcp: None,
            permission_mode,
        })
        .await
    }

    /// Full-access review session, separate from restricted analysis used by other features.
    async fn create_review_session(
        &self,
        directory: &str,
        additional: &[String],
    ) -> Result<SessionHandle, DriverError> {
        let permission_mode = self
            .permission_catalog(Some(directory))
            .await?
            .review_mode();
        self.create_session(SessionRequest {
            working_directory: directory.into(),
            additional_directories: additional.to_vec(),
            history_mcp: None,
            permission_mode,
        })
        .await
    }
    fn review_prompt_permission_mode(&self) -> Option<&str> {
        None
    }

    async fn read_subagent(&self, _id: &str) -> Result<Vec<Value>, DriverError> {
        Err(DriverError::CapabilityUnavailable(
            "subagent history unavailable".into(),
        ))
    }
    async fn resume_workspace_session(
        &self,
        id: &str,
        workspace: SessionRequest,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.resume_session_with_model(id, &workspace.working_directory, model)
            .await
    }
    async fn permission_catalog(
        &self,
        _workspace: Option<&str>,
    ) -> Result<PermissionCatalog, DriverError> {
        Ok(PermissionCatalog::default())
    }
    async fn model_catalog_in_workspace(
        &self,
        model: Option<&str>,
        _workspace: Option<&str>,
    ) -> Result<ModelCatalog, DriverError> {
        self.model_catalog_for(model).await
    }
    async fn model_catalog_for(&self, _model: Option<&str>) -> Result<ModelCatalog, DriverError> {
        self.model_catalog().await
    }
    async fn model_catalog(&self) -> Result<ModelCatalog, DriverError> {
        Ok(ModelCatalog {
            models: self
                .available_models()
                .await?
                .iter()
                .map(|id| ModelOption::new(id, id))
                .collect(),
            default_model: None,
        })
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        if thinking.is_some() {
            return Err(DriverError::CapabilityUnavailable(
                "thinking selection unavailable".into(),
            ));
        }
        self.create_session_with_model(request, model).await
    }
    async fn resume_selected_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        if thinking.is_some() {
            return Err(DriverError::CapabilityUnavailable(
                "thinking selection unavailable".into(),
            ));
        }
        self.resume_workspace_session(id, request, model).await
    }
    /// Apply a changed selection while retaining the native conversation.
    /// `CapabilityUnavailable` means the hub may rebuild; transport/protocol failures
    /// must remain visible instead of silently starting a different conversation.
    async fn reconfigure_session(
        &self,
        id: &str,
        request: SessionRequest,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        // Native load APIs often interpret null as "keep current", not reset.
        // Resolve reset selections from the configured model catalog explicitly.
        if model.is_some() && thinking.is_some() {
            return self
                .resume_selected_session(id, request, model, thinking)
                .await;
        }
        let catalog = self
            .model_catalog_in_workspace(model, Some(&request.working_directory))
            .await?;
        let model = model.or(catalog.default_model.as_deref()).ok_or_else(|| {
            DriverError::CapabilityUnavailable("native default model cannot be resolved".into())
        })?;
        let option = catalog
            .models
            .iter()
            .find(|option| option.id == model)
            .ok_or_else(|| {
                DriverError::CapabilityUnavailable("selected model cannot be resolved".into())
            })?;
        let thinking = thinking.or(option.default_thinking.as_deref());
        if thinking.is_none() && !option.thinking_options.is_empty() {
            return Err(DriverError::CapabilityUnavailable(
                "native default thinking cannot be resolved".into(),
            ));
        }
        self.resume_selected_session(id, request, Some(model), thinking)
            .await
    }
    async fn available_models(&self) -> Result<Vec<String>, DriverError> {
        Ok(vec![])
    }

    async fn create_session_with_model(
        &self,
        request: SessionRequest,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        if model.is_some() {
            return Err(DriverError::CapabilityUnavailable(
                "explicit model selection unavailable".into(),
            ));
        }
        self.create_session(request).await
    }

    async fn resume_session_with_model(
        &self,
        id: &str,
        cwd: &str,
        model: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        if model.is_some() {
            return Err(DriverError::CapabilityUnavailable(
                "explicit model selection unavailable".into(),
            ));
        }
        self.resume_session(id, cwd).await
    }
    /// Exact Provider + Installation identity owned by this adapter instance.
    fn executor(&self) -> &ExecutorRef;

    fn provider_id(&self) -> &ProviderId {
        &self.executor().provider_id
    }

    fn driver_version(&self) -> &'static str;

    fn native_agent_version(&self) -> Option<&str> {
        None
    }

    /// Keeps the owned native process/thread available between turns in this Hub process.
    /// This does not imply that the session can be restored after an app restart.
    /// The prompt stream may retain child events after its one Turn terminal.
    fn supports_background_events(&self) -> bool {
        false
    }

    /// Take the session's unsolicited root-turn stream once. Each continuation
    /// starts with `TurnStarted` and ends with a native terminal event. Child
    /// settlements remain on the originating prompt stream. Taking this stream
    /// never submits input or starts model work.
    async fn take_autonomous_events(&self, _handle: &SessionHandle) -> Option<DriverEventStream> {
        None
    }

    /// Covers the short interval before unsolicited execution is persisted by Core.
    async fn has_autonomous_turn(&self, _native_session_id: &str) -> bool {
        false
    }

    /// Core has persisted one continuation's terminal. Until then the driver
    /// must keep new user input out, even if the native result already arrived.
    /// Only the original scope may acknowledge its persisted continuation.
    /// Native IDs can be reused by a replacement actor.
    async fn acknowledge_autonomous_turn(&self, _handle: &SessionHandle) {}

    fn supports_live_session_reuse(&self) -> bool {
        false
    }

    fn resume_capability(&self) -> SessionResumeCapability {
        SessionResumeCapability::Unknown
    }

    async fn create_session(&self, request: SessionRequest) -> Result<SessionHandle, DriverError>;

    /// Snapshot existing local ownership for explicit management, never restore
    /// or spawn. Callers must retain this handle rather than reconstruct it.
    async fn current_session_handle(&self, native_session_id: &str) -> Option<SessionHandle> {
        Some(SessionHandle::new(native_session_id.to_owned()))
    }

    async fn resume_session(
        &self,
        _native_session_id: &str,
        _working_directory: &str,
    ) -> Result<SessionHandle, DriverError> {
        Err(DriverError::CapabilityUnavailable(
            "native session resume is not implemented".into(),
        ))
    }

    /// Starts one native turn. Implementations must deliver every `request.context` item in order
    /// before the user `request.prompt`; returning `Ok` means the complete request was accepted.
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError>;

    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError>;

    /// Answers one pending approval previously surfaced through
    /// [`DriverEvent::ApprovalRequested`]. `option_id` must be one of the
    /// vendor-provided option ids from that event; hubs never invent options.
    async fn respond_to_interaction(
        &self,
        _handle: &SessionHandle,
        _native_request_id: &str,
        _option_id: &str,
    ) -> Result<(), DriverError> {
        Err(DriverError::CapabilityUnavailable(
            "interactive approvals are not implemented for this driver".into(),
        ))
    }

    async fn respond_to_question(
        &self,
        _handle: &SessionHandle,
        _request_id: &str,
        _answer: agent_domain::QuestionAnswer,
    ) -> Result<(), DriverError> {
        Err(DriverError::CapabilityUnavailable(
            "structured questions are not supported".into(),
        ))
    }

    /// Releases the live connection/process handle without deleting a resumable native session.
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError>;

    /// Must not send requests, re-resolve a native ID later, or create a session.
    /// Adapters with asynchronous retirement expose the original positive proof
    /// so a caller can keep observing after shutdown reports an uncertain result.
    async fn shutdown_proof(&self, _handle: &SessionHandle) -> Option<SessionShutdownProof> {
        None
    }
}

/// Deterministic driver used by the first vertical slice and offline tests.
pub struct FakeDriver {
    executor: ExecutorRef,
    received_requests: tokio::sync::Mutex<Vec<PromptRequest>>,
    resumed_sessions: tokio::sync::Mutex<Vec<String>>,
    fail_next_prompt: AtomicBool,
    fail_next_resume: std::sync::Mutex<Option<DriverError>>,
}

impl FakeDriver {
    pub fn new(provider_id: &str) -> Result<Self, agent_domain::DomainError> {
        Ok(Self::with_executor(ExecutorRef {
            provider_id: ProviderId::parse(provider_id)?,
            installation_id: None,
        }))
    }

    /// Construct an offline driver with the installation identity held by its fixture.
    #[must_use]
    pub fn with_executor(executor: ExecutorRef) -> Self {
        Self {
            executor,
            received_requests: tokio::sync::Mutex::new(vec![]),
            resumed_sessions: tokio::sync::Mutex::new(vec![]),
            fail_next_prompt: AtomicBool::new(false),
            fail_next_resume: std::sync::Mutex::new(None),
        }
    }

    pub async fn received_requests(&self) -> Vec<PromptRequest> {
        self.received_requests.lock().await.clone()
    }

    pub async fn resumed_sessions(&self) -> Vec<String> {
        self.resumed_sessions.lock().await.clone()
    }

    pub fn fail_next_prompt(&self) {
        self.fail_next_prompt.store(true, Ordering::Release);
    }

    /// Makes the next session-resume attempt fail with `error`.
    ///
    /// # Panics
    ///
    /// Panics if the fake driver's failure-injection lock was poisoned by a
    /// previous panic.
    pub fn fail_next_resume(&self, error: DriverError) {
        *self
            .fail_next_resume
            .lock()
            .expect("fake resume failure lock poisoned") = Some(error);
    }
}

#[async_trait]
impl AgentDriver for FakeDriver {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }

    fn driver_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn resume_capability(&self) -> SessionResumeCapability {
        SessionResumeCapability::Supported
    }

    async fn create_session(&self, _request: SessionRequest) -> Result<SessionHandle, DriverError> {
        Ok(SessionHandle::new(format!("fake_{}", ulid::Ulid::new())))
    }

    async fn resume_session(
        &self,
        native_session_id: &str,
        _working_directory: &str,
    ) -> Result<SessionHandle, DriverError> {
        if let Some(error) = self
            .fail_next_resume
            .lock()
            .expect("fake resume failure lock poisoned")
            .take()
        {
            return Err(error);
        }
        if !native_session_id.starts_with("fake_") {
            return Err(DriverError::Protocol(
                "fake session id has an invalid product signature".into(),
            ));
        }
        self.resumed_sessions
            .lock()
            .await
            .push(native_session_id.into());
        Ok(SessionHandle::new(native_session_id.into()))
    }

    async fn prompt(
        &self,
        _handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        if self.fail_next_prompt.swap(false, Ordering::AcqRel) {
            return Err(DriverError::Protocol(
                "injected prompt rejection before request acceptance".into(),
            ));
        }
        self.received_requests.lock().await.push(request.clone());
        let item_id = format!("msg_{}", ulid::Ulid::new());
        let events = vec![
            Ok(DriverEvent::TurnStarted {
                native_turn_id: Some(format!("fake_turn_{}", ulid::Ulid::new())),
            }),
            Ok(DriverEvent::TextDelta {
                item_id: item_id.clone(),
                text: "Fake agent received: ".into(),
            }),
            Ok(DriverEvent::TextDelta {
                item_id,
                text: request.prompt,
            }),
            Ok(DriverEvent::TurnCompleted {
                summary: "Fake run completed".into(),
            }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }

    async fn interrupt(&self, _handle: &SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }

    async fn shutdown(&self, _handle: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::explicitly_reports_quota_exhaustion;

    #[test]
    fn session_authority_survives_cloning_but_never_serialization_or_id_reuse() {
        let original = super::SessionHandle::scoped("vendor-reuses-this-id".into());
        assert!(original.same_scope(&original.clone()));
        assert!(!original.same_scope(&super::SessionHandle::scoped(
            original.native_session_id.clone()
        )));
        assert!(!original.same_scope(&super::SessionHandle::new(
            original.native_session_id.clone()
        )));
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"nativeSessionId":"vendor-reuses-this-id"})
        );
        let decoded: super::SessionHandle = serde_json::from_value(json).unwrap();
        assert!(!decoded.same_scope(&original));
        let mut changed = original.clone();
        changed.native_session_id = "another-session".into();
        assert!(!changed.same_scope(&original));
    }

    #[test]
    fn quota_classifier_requires_explicit_exhaustion_language() {
        assert!(explicitly_reports_quota_exhaustion(
            "The usage limit has been reached"
        ));
        assert!(explicitly_reports_quota_exhaustion(
            "Usage limit reached; retry after reset"
        ));
        assert!(explicitly_reports_quota_exhaustion(
            "insufficient credits for this request"
        ));
        assert!(explicitly_reports_quota_exhaustion(
            "subscription:free-usage-exhausted: You've used all the included free usage for model grok-4.6 for now."
        ));
        assert!(!explicitly_reports_quota_exhaustion(
            "HTTP 429 rate limit: retry in 2 seconds"
        ));
        assert!(!explicitly_reports_quota_exhaustion(
            "request failed while reading usage limits"
        ));
    }
}
