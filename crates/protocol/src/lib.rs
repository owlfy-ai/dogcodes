//! Transport-neutral command, event, and query data transfer objects.

pub mod submission;

use agent_domain::{
    ArtifactId, CommandId, CorrelationId, DeviceId, EventId, ExecutorRef, InstallationId,
    ProjectId, RunId, RunState, SessionMapId, TaskId, TurnId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u16 = 1;

/// Identity and original metadata for a managed attachment upload. No file bytes
/// are needed to query its receipt after a disconnected client reopens.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentUploadRequest {
    pub upload_id: String,
    pub name: String,
    pub mime_type: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryCode {
    InstallationUnconfigured,
    InstallationBroken,
    InstallationUnsupported,
    InstallationNotReady,
    DriverUnavailable,
    RunOwnershipUnknown,
    RunInterrupting,
    ApprovalPending,
    SessionUnavailable,
}

/// Allowlisted operational metadata only: no prompts, paths, native session
/// IDs, raw provider errors, tool inputs, account details, or credentials.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryItem {
    pub code: RecoveryCode,
    pub provider_id: Option<String>,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    pub turn_id: Option<TurnId>,
    pub expected_stream_version: Option<u64>,
    pub can_cancel: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverySnapshot {
    pub schema_version: u16,
    pub generated_at: DateTime<Utc>,
    pub last_sequence: u64,
    pub truncated: bool,
    pub items: Vec<RecoveryItem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ActorRef {
    LocalUser,
    SystemRecovery,
    PairedDevice(DeviceId),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandEnvelope<T> {
    pub schema_version: u16,
    pub command_id: CommandId,
    pub idempotency_key: String,
    pub actor: ActorRef,
    pub expected_stream_version: Option<u64>,
    pub issued_at: DateTime<Utc>,
    pub payload: T,
}

impl<T> CommandEnvelope<T> {
    #[must_use]
    pub fn local(payload: T) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            command_id: CommandId::new(),
            idempotency_key: CommandId::new().to_string(),
            actor: ActorRef::LocalUser,
            expected_stream_version: None,
            issued_at: Utc::now(),
            payload,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandAck {
    /// Logical request identity; absent in older persisted receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_fingerprint: Option<String>,
    pub command_id: CommandId,
    pub accepted: bool,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamType {
    Project,
    Task,
    Installation,
    Quota,
    System,
}

impl StreamType {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Task => "task",
            Self::Installation => "installation",
            Self::Quota => "quota",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamRef {
    pub stream_type: StreamType,
    pub stream_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum EventSource {
    Hub,
    Driver {
        provider_id: String,
        source_sequence: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub schema_version: u16,
    pub sequence: u64,
    pub event_id: EventId,
    pub stream: StreamRef,
    pub stream_version: u64,
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub correlation_id: CorrelationId,
    pub causation_id: Option<String>,
    pub source: EventSource,
    pub payload: Value,
    pub vendor_extensions: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum EventStreamMessage {
    ReplayStarted {
        after_sequence: u64,
    },
    Event(Box<EventEnvelope>),
    CaughtUp {
        last_sequence: u64,
    },
    Error {
        message: String,
        retryable: bool,
        last_sequence: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewEvent {
    pub event_id: EventId,
    pub stream: StreamRef,
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: CorrelationId,
    pub causation_id: Option<String>,
    pub source: EventSource,
    pub payload: Value,
    pub vendor_extensions: Option<Value>,
}

impl NewEvent {
    #[must_use]
    pub fn hub(stream: StreamRef, event_type: impl Into<String>, payload: Value) -> Self {
        Self {
            event_id: EventId::new(),
            stream,
            event_type: event_type.into(),
            occurred_at: Utc::now(),
            correlation_id: CorrelationId::new(),
            causation_id: None,
            source: EventSource::Hub,
            payload,
            vendor_extensions: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum CoreCommand {
    SetAgentPreferences(AgentPreferences),
    SetHistoryPreferences(HistoryPreferences),
    RegisterProject(RegisterProject),
    UpdateProject(RegisterProject),
    CreateTask(CreateTask),
    RenameTask(RenameTask),
    DeleteTask(TaskLifecycle),
    RestoreTask(TaskLifecycle),
    SetPreferredExecutor(SetPreferredExecutor),
    ConfirmContextPackage(ConfirmContextPackage),
    OptOutReconciliation(OptOutReconciliation),
    SubmitTurn(SubmitTurn),
    CancelTurn(CancelTurn),
    RespondToApproval(RespondToApproval),
    CaptureWorkspaceDiff(CaptureWorkspaceDiff),
    RecordTestResult(RecordTestResult),
    RecordInstallationProbe(RecordInstallationProbe),
    ReconcileMissingInstallation(ReconcileMissingInstallation),
}

/// Shared history organization. Collapsed panels and drafts are not business state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPreferences {
    pub tasks: std::collections::BTreeMap<String, HistoryTaskState>,
    pub projects: std::collections::BTreeMap<String, HistoryProjectState>,
    pub sorts: std::collections::BTreeMap<String, HistorySort>,
    pub orders: std::collections::BTreeMap<String, Vec<String>>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryTaskState {
    Pinned,
    Archived,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryProjectState {
    Pinned,
    Removed,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistorySort {
    Manual,
    Updated,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPreferencesSnapshot {
    pub revision: u64,
    pub last_sequence: u64,
    pub preferences: HistoryPreferences,
}

/// Host-wide business settings; drafts and visual layout are deliberately absent.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPreferences {
    pub order: Vec<String>,
    pub disabled: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPreferencesSnapshot {
    /// Zero means no settings have been saved or imported on this host yet.
    pub revision: u64,
    /// Atomic event head for a gap-free subscription after this snapshot.
    pub last_sequence: u64,
    pub preferences: AgentPreferences,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLifecycle {
    pub task_id: TaskId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameTask {
    pub task_id: TaskId,
    pub title: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterProject {
    #[serde(default)]
    pub additional_directories: Vec<String>,
    pub project_id: ProjectId,
    pub display_name: String,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTask {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub title: String,
    pub preferred_executor: ExecutorRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPreferredExecutor {
    pub task_id: TaskId,
    pub executor: ExecutorRef,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmContextPackage {
    pub request: ContextPreviewRequest,
    pub expected_payload_sha256: String,
    /// The user's edited draft. When omitted, the deterministic Hub draft is confirmed. Core
    /// always rebuilds the Hub draft, validates immutable routing/provenance fields, and applies
    /// redaction again; this draft is never stored directly as an Event or Artifact.
    pub edited_package: Option<Box<ContextPackageV1>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEditRequest {
    pub request: ContextPreviewRequest,
    pub base_payload_sha256: String,
    pub edited_package: ContextPackageV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptOutReconciliation {
    pub request: ContextPreviewRequest,
    pub expected_payload_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
/// Model and thinking are the next turn's complete selection; null uses native defaults.
pub struct SubmitTurn {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_fix: Option<ReviewFixRef>,
    #[serde(default)]
    pub attachments: Vec<agent_domain::PromptAttachment>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub task_id: TaskId,
    pub prompt: String,
    pub executor_override: Option<ExecutorRef>,
}

/// A user-selected subset of a frozen, terminal review report.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFixRef {
    pub run_id: RunId,
    pub input_sha256: String,
    pub finding_indices: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelTurn {
    pub task_id: TaskId,
    pub turn_id: TurnId,
    pub run_id: RunId,
}

/// Answers one vendor approval previously surfaced as `approval.requested`.
/// `approval_id` is the agent's native request id; `optionId` must be one of
/// the vendor-provided option ids from that request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RespondToApproval {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<agent_domain::QuestionAnswer>,
    pub task_id: TaskId,
    pub turn_id: TurnId,
    pub requested_at_sequence: u64,
    pub approval_id: String,
    pub option_id: String,
}

/// One pending vendor approval projected from the task stream.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<agent_domain::QuestionForm>,
    pub approval_id: String,
    pub turn_id: TurnId,
    pub tool_name: String,
    pub command_summary: String,
    pub options: Vec<PendingApprovalOption>,
    pub requested_at_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApprovalOption {
    pub option_id: String,
    pub label: String,
    pub semantic_role: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureWorkspaceDiff {
    pub task_id: TaskId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordTestResult {
    pub task_id: TaskId,
    pub command: String,
    pub outcome: String,
    pub report: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReportV1 {
    pub schema_version: u16,
    pub command: String,
    pub outcome: String,
    pub report: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordInstallationProbe {
    pub provider_id: String,
    pub executable: String,
    pub fingerprint: String,
    pub status: InstallationStatus,
    pub version: Option<String>,
    pub version_text: Option<String>,
    pub diagnostic: Option<String>,
    pub elapsed_millis: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileMissingInstallation {
    pub installation_id: InstallationId,
    pub provider_id: String,
    pub executable: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationStatus {
    Installed,
    NotConfigured,
    Ready,
    UnsupportedVersion,
    Broken,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationSummary {
    pub installation_id: InstallationId,
    pub provider_id: String,
    pub executable: String,
    pub fingerprint: String,
    pub status: InstallationStatus,
    pub version: Option<String>,
    pub version_text: Option<String>,
    pub diagnostic: Option<String>,
    pub probe_elapsed_millis: u64,
    pub probed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub stream_version: u64,
    pub additional_directories: Vec<String>,
    pub project_id: ProjectId,
    pub display_name: String,
    pub path: String,
    pub task_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRunSummary {
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub executor: ExecutorRef,
    pub state: RunState,
    pub started_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub title: String,
    pub preferred_executor: ExecutorRef,
    #[serde(default)]
    pub last_turn_executor: Option<ExecutorRef>,
    pub active_primary_run: Option<ActiveRunSummary>,
    pub stream_version: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeCapability {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMapStatus {
    Active,
    Resumable,
    Missing,
    Incompatible,
    Detached,
    RebuildPending,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionMapSummary {
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub session_map_id: SessionMapId,
    pub task_id: TaskId,
    pub executor: ExecutorRef,
    pub native_session_id: String,
    pub last_native_turn_id: Option<String>,
    pub workspace_locator: String,
    pub driver_version: Option<String>,
    pub native_agent_version: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub resume_capability: ResumeCapability,
    pub status: SessionMapStatus,
    pub last_seen_sequence: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completeness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens_per_second: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_request_count: Option<u64>,
    pub scope: String,
    pub request_count: Option<u64>,
    pub models: Vec<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub credits: Option<f64>,
    pub partial: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_output_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TurnUsage>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_completion: Option<TurnCompletion>,
    pub attachments: Vec<agent_domain::PromptAttachment>,
    pub item_id: String,
    pub task_id: TaskId,
    pub turn_id: Option<TurnId>,
    pub kind: String,
    pub text: String,
    pub provider_id: Option<String>,
    pub sequence: u64,
    pub occurred_at: DateTime<Utc>,
}

/// A timeline and its canonical cursor, read from the same database snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineSnapshot {
    pub items: Vec<TimelineItem>,
    pub last_sequence: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextPackageKind {
    Handoff,
    Reconciliation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextStrength {
    Compact,
    Standard,
    Full,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextDecision {
    pub statement: String,
    pub source_ref: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelevantFile {
    pub path: String,
    pub status: String,
    pub content_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub base: String,
    pub head: String,
    pub diff_stat: String,
    pub artifact_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorkspaceSnapshotV1 {
    pub schema_version: u16,
    pub repo_root: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub worktree_identity: String,
    pub status_format: String,
    pub status_fingerprint: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDiagnosticKind {
    WorkspaceUnavailable,
    NotRepository,
    TimedOut,
    CommandFailed,
    OutputTruncated,
    PathBudgetExceeded,
    WorkspaceChanged,
    ExternalFilterConfigured,
    InvalidOutput,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCollectionDiagnostic {
    pub kind: WorkspaceDiagnosticKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSnapshotPhase {
    TurnStart,
    TurnEnd,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshotSummary {
    pub task_id: TaskId,
    pub turn_id: TurnId,
    pub run_id: RunId,
    pub phase: WorkspaceSnapshotPhase,
    pub sequence: u64,
    pub captured_at: DateTime<Utc>,
    pub git: Option<GitWorkspaceSnapshotV1>,
    pub change_summary: Option<ChangeSummary>,
    pub relevant_file_count: u32,
    pub dependency_change_count: u32,
    pub diagnostic: Option<WorkspaceDiagnosticKind>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTestResult {
    pub command: String,
    pub outcome: String,
    pub artifact_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPointer {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub locator: String,
    pub content_hash: String,
    pub trigger_hints: Vec<String>,
    pub byte_length: u64,
    pub sensitivity: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextProvenance {
    pub builder_version: u16,
    pub built_at: DateTime<Utc>,
    pub redaction_ruleset_version: u16,
    pub redaction_count: u32,
    pub source_after_sequence: u64,
    pub source_through_sequence: u64,
    pub source_sequences: Vec<u64>,
}

/// Exact stored fact bytes offered to a native session; not semantic acknowledgement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFactDelivery {
    pub sequence: u64,
    pub field: String,
    pub sha256: String,
    pub offset: usize,
    pub end: usize,
    pub total_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticSummary {
    pub text: String,
    pub source_run_id: RunId,
    pub provider_id: String,
    pub model: Option<String>,
    pub input_source_sequences: Vec<u64>,
    pub output_artifact_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPackageV1 {
    pub delivery_facts: Vec<HistoryFactDelivery>,
    /// Original conversation text, never a generated summary.
    pub conversation_text: String,
    pub schema_version: u16,
    pub kind: ContextPackageKind,
    pub strength: ContextStrength,
    pub source_executor: ExecutorRef,
    pub target_executor: ExecutorRef,
    pub objective: String,
    pub user_constraints: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub completed_work: Vec<String>,
    pub decisions: Vec<ContextDecision>,
    pub pending_work: Vec<String>,
    pub relevant_files: Vec<RelevantFile>,
    pub dependency_changes: Vec<RelevantFile>,
    pub change_summary: ChangeSummary,
    pub tests: Vec<ContextTestResult>,
    pub recent_turn_refs: Vec<String>,
    pub context_pointers: Vec<ContextPointer>,
    pub semantic_summary: Option<SemanticSummary>,
    pub provenance: ContextProvenance,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltContextPackage {
    pub package: ContextPackageV1,
    pub payload_sha256: String,
    pub byte_length: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPreviewRequest {
    pub task_id: TaskId,
    pub kind: ContextPackageKind,
    pub strength: ContextStrength,
    pub source_executor: ExecutorRef,
    pub target_executor: ExecutorRef,
    pub after_sequence: u64,
    pub through_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactMetadata {
    pub artifact_id: ArtifactId,
    pub media_type: String,
    pub payload_sha256: String,
    pub byte_length: u64,
    pub created_at: DateTime<Utc>,
    pub redaction_ruleset_version: u16,
    pub redaction_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactCursor {
    pub sequence: u64,
    pub artifact_id: ArtifactId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifactSummary {
    pub metadata: ArtifactMetadata,
    pub source_sequence: u64,
    pub source_kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPage {
    pub items: Vec<TaskArtifactSummary>,
    pub next_cursor: Option<ArtifactCursor>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactReadRequest {
    pub task_id: TaskId,
    pub artifact_id: ArtifactId,
    pub expected_payload_sha256: String,
    pub offset: u64,
    pub limit: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactChunk {
    pub metadata: ArtifactMetadata,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub next_offset: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactExportRequest {
    pub task_id: TaskId,
    pub artifact_id: ArtifactId,
    pub expected_payload_sha256: String,
    pub confirmed_sensitive_export: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapSnapshot {
    pub projects: Vec<ProjectSummary>,
    pub tasks: Vec<TaskSummary>,
    #[serde(default)]
    pub deleted_tasks: Vec<TaskSummary>,
    pub installations: Vec<InstallationSummary>,
    pub active_runs: Vec<ActiveRunSummary>,
    pub session_maps: Vec<AgentSessionMapSummary>,
    pub last_sequence: u64,
    pub companion: CompanionProjection,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionProjection {
    pub schema_version: u16,
    pub revision: u64,
    pub generated_at: DateTime<Utc>,
    pub stale: bool,
    pub workspaces: Vec<CompanionWorkspace>,
    pub recent_completion: Option<RecentCompletion>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionWorkspace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_terminal: Option<CompanionTerminal>,
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub display_name: String,
    pub preferred_executor: ExecutorRef,
    pub active_primary_run: Option<ActiveRunSummary>,
    pub secondary_agent_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<CompanionReview>,
    pub pending_interaction_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionReview {
    pub run_id: RunId,
    pub provider_id: String,
    pub kind: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub completed_groups: u32,
    pub total_groups: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionTerminal {
    pub provider_id: Option<String>,
    pub state: RunState,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentCompletion {
    pub task_id: TaskId,
    pub run_id: RunId,
    pub provider_id: String,
    pub completed_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    Conflict,
    CapabilityUnavailable,
    AgentNotConfigured,
    AgentVersionUnsupported,
    AgentDisconnected,
    PermissionRequestExpired,
    WorkspaceBusy,
    WorkspaceMaterializationFailed,
    AdoptionConflict,
    StorageUnavailable,
    ResourceExhausted,
    Cancelled,
    Internal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub correlation_id: CorrelationId,
    pub details: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_stream_control_fields_use_camel_case() {
        let value = serde_json::to_value(EventStreamMessage::CaughtUp { last_sequence: 42 })
            .expect("stream message should serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "type": "caught_up",
                "data": { "lastSequence": 42 }
            })
        );
    }

    #[test]
    fn driver_source_fields_use_camel_case() {
        let value = serde_json::to_value(EventSource::Driver {
            provider_id: "codex".into(),
            source_sequence: Some("7".into()),
        })
        .expect("event source should serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "driver",
                "providerId": "codex",
                "sourceSequence": "7"
            })
        );
    }

    #[test]
    fn not_configured_installation_status_has_a_stable_wire_value() {
        let value = serde_json::to_value(InstallationStatus::NotConfigured)
            .expect("installation status should serialize");
        assert_eq!(value, serde_json::json!("not_configured"));
    }

    #[test]
    fn context_package_enums_have_stable_wire_values() {
        assert_eq!(
            serde_json::to_value(ContextPackageKind::Reconciliation)
                .expect("context package kind should serialize"),
            serde_json::json!("reconciliation")
        );
        assert_eq!(
            serde_json::to_value(ContextStrength::Standard)
                .expect("context strength should serialize"),
            serde_json::json!("standard")
        );
        assert_eq!(
            serde_json::to_value(ResumeCapability::Unknown)
                .expect("resume capability should serialize"),
            serde_json::json!("unknown")
        );
        assert_eq!(
            serde_json::to_value(SessionMapStatus::Detached)
                .expect("session map status should serialize"),
            serde_json::json!("detached")
        );
    }

    #[test]
    fn git_workspace_snapshot_and_diagnostic_have_stable_wire_values() {
        let value = serde_json::to_value(GitWorkspaceSnapshotV1 {
            schema_version: 1,
            repo_root: "/workspace".into(),
            head: Some("abc123".into()),
            branch: Some("main".into()),
            detached: false,
            worktree_identity: "sha256:worktree".into(),
            status_format: "porcelain_v2".into(),
            status_fingerprint: "snapshot".into(),
        })
        .expect("git workspace snapshot should serialize");
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["repoRoot"], "/workspace");
        assert_eq!(value["worktreeIdentity"], "sha256:worktree");
        assert_eq!(value["statusFormat"], "porcelain_v2");
        assert_eq!(
            serde_json::to_value(WorkspaceDiagnosticKind::PathBudgetExceeded)
                .expect("workspace diagnostic should serialize"),
            serde_json::json!("path_budget_exceeded")
        );
        assert_eq!(
            serde_json::to_value(WorkspaceDiagnosticKind::WorkspaceChanged)
                .expect("workspace drift diagnostic should serialize"),
            serde_json::json!("workspace_changed")
        );
        assert_eq!(
            serde_json::to_value(WorkspaceDiagnosticKind::ExternalFilterConfigured)
                .expect("filter diagnostic should serialize"),
            serde_json::json!("external_filter_configured")
        );
    }

    #[test]
    fn artifact_requests_accept_only_typed_ids_and_declared_fields() {
        let valid = serde_json::json!({ "taskId": TaskId::new(), "artifactId": ArtifactId::new(),
            "expectedPayloadSha256": "a".repeat(64), "offset": 0, "limit": 32768 });
        assert!(serde_json::from_value::<ArtifactReadRequest>(valid.clone()).is_ok());
        let mut invalid = valid.clone();
        invalid["artifactId"] = serde_json::json!("../../app.db");
        assert!(serde_json::from_value::<ArtifactReadRequest>(invalid).is_err());
        let mut invalid = valid;
        invalid["path"] = serde_json::json!("/tmp/arbitrary");
        assert!(serde_json::from_value::<ArtifactReadRequest>(invalid).is_err());
        let export = serde_json::json!({ "taskId": TaskId::new(), "artifactId": ArtifactId::new(),
            "expectedPayloadSha256": "a".repeat(64), "confirmedSensitiveExport": true,
            "path": "/tmp/arbitrary" });
        assert!(serde_json::from_value::<ArtifactExportRequest>(export).is_err());
    }
}
