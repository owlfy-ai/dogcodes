// Wire mirror of agent-protocol. Kept deliberately transport-neutral.
export const SCHEMA_VERSION = 1 as const;

export type ProjectId = `prj_${string}`;
export type TaskId = `tsk_${string}`;
export type TurnId = `trn_${string}`;
export type RunId = `run_${string}`;
export type ArtifactId = `art_${string}`;

export interface ArtifactMetadata {
  artifactId: ArtifactId;
  mediaType: string;
  payloadSha256: string;
  byteLength: number;
  createdAt: string;
  redactionRulesetVersion: number;
  redactionCount: number;
}
export interface ArtifactCursor { sequence: number; artifactId: ArtifactId }
export interface TaskArtifactSummary {
  metadata: ArtifactMetadata;
  sourceSequence: number;
  sourceKind: string;
}
export interface ArtifactPage { items: TaskArtifactSummary[]; nextCursor: ArtifactCursor | null }
export interface ArtifactReadRequest {
  taskId: TaskId;
  artifactId: ArtifactId;
  expectedPayloadSha256: string;
  offset: number;
  limit: number;
}
export interface ArtifactChunk {
  metadata: ArtifactMetadata;
  offset: number;
  bytes: number[];
  nextOffset: number | null;
}
export interface ArtifactExportRequest {
  taskId: TaskId;
  artifactId: ArtifactId;
  expectedPayloadSha256: string;
  confirmedSensitiveExport: boolean;
}

export interface ExecutorRef {
  providerId: string;
  installationId: string | null;
}

/** Host-wide settings; per-device drafts and visual layout remain local. */
export interface AgentPreferences { order: string[]; disabled: string[] }
export interface AgentPreferencesSnapshot { revision: number; lastSequence: number; preferences: AgentPreferences }

export interface HistoryPreferences {
  tasks: Record<string, "pinned" | "archived">;
  projects: Record<string, "pinned" | "removed">;
  sorts: Partial<Record<"pinned" | "projects" | "archived", "manual" | "updated">>;
  orders: Record<string, string[]>;
}
export interface HistoryPreferencesSnapshot { revision: number; lastSequence: number; preferences: HistoryPreferences }

export type RunState =
  | "queued"
  | "starting"
  | "running"
  | "waiting_for_input"
  | "waiting_for_approval"
  | "interrupting"
  | "interrupted"
  | "completed"
  | "failed"
  | "disconnected"
  | "reconnecting";

export interface ActiveRunSummary {
  runId: RunId;
  turnId: TurnId;
  executor: ExecutorRef;
  state: RunState;
  startedAt: string;
}

export interface ProjectSummary {
  updatedAt?: string | null;
  streamVersion?: number;
  additionalDirectories: string[];
  projectId: ProjectId;
  displayName: string;
  path: string;
  taskCount: number;
}

export interface TaskSummary {
  updatedAt?: string | null;
  taskId: TaskId;
  projectId: ProjectId;
  title: string;
  preferredExecutor: ExecutorRef;
  /** Executor captured when the most recent primary turn was submitted. */
  lastTurnExecutor?: ExecutorRef | null;
  activePrimaryRun: ActiveRunSummary | null;
  streamVersion: number;
}

export type ResumeCapability = "supported" | "unsupported" | "unknown";
export type SessionMapStatus = "active" | "resumable" | "missing" | "incompatible" | "detached" | "rebuild_pending";

export interface AgentSessionMapSummary {
  thinking: string | null;
  model: string | null;
  sessionMapId: `ses_${string}`;
  taskId: TaskId;
  executor: ExecutorRef;
  nativeSessionId: string;
  lastNativeTurnId: string | null;
  workspaceLocator: string;
  driverVersion: string | null;
  nativeAgentVersion: string | null;
  createdAt: string;
  lastUsedAt: string;
  resumeCapability: ResumeCapability;
  status: SessionMapStatus;
  lastSeenSequence: number;
}

export type InstallationStatus =
  | "installed"
  | "not_configured"
  | "ready"
  | "unsupported_version"
  | "broken";

export interface InstallationSummary {
  installationId: `ins_${string}`;
  providerId: string;
  executable: string;
  fingerprint: string;
  status: InstallationStatus;
  version: string | null;
  versionText: string | null;
  diagnostic: string | null;
  probeElapsedMillis: number;
  probedAt: string;
}

export interface AttachmentIntegrity { sha256: string; byteLength: number }
export interface AttachmentUploadRequest extends AttachmentIntegrity { uploadId: string; name: string; mimeType: string }
export interface PromptAttachment { name: string; path: string; mimeType: string; integrity?: AttachmentIntegrity }

export interface TimelineItem {
  /** Present for a terminal turn (completed, failed, or disconnected); derived from canonical events. */
  turnCompletion?: { startedAt: string; completedAt: string; usage?: TurnUsage | null; firstOutputMs?: number | null };
  attachments: PromptAttachment[];
  itemId: string;
  taskId: TaskId;
  turnId: TurnId | null;
  kind: string;
  text: string;
  providerId: string | null;
  sequence: number;
  occurredAt: string;
}

export interface TimelineSnapshot { items: TimelineItem[]; lastSequence: number }

export type ContextPackageKind = "handoff" | "reconciliation";
export type ContextStrength = "compact" | "standard" | "full";

export interface ContextDecision {
  statement: string;
  sourceRef: string;
}

export interface RelevantFile {
  path: string;
  status: string;
  contentHash: string | null;
}

export interface ChangeSummary {
  base: string;
  head: string;
  diffStat: string;
  artifactRef: string | null;
}

export interface GitWorkspaceSnapshotV1 {
  schemaVersion: 1;
  repoRoot: string;
  head: string | null;
  branch: string | null;
  detached: boolean;
  worktreeIdentity: string;
  statusFormat: "porcelain_v2";
  statusFingerprint: string;
}

export type WorkspaceDiagnosticKind =
  | "workspace_unavailable"
  | "not_repository"
  | "timed_out"
  | "command_failed"
  | "output_truncated"
  | "path_budget_exceeded"
  | "workspace_changed"
  | "external_filter_configured"
  | "invalid_output";

export type WorkspaceSnapshotPhase = "turn_start" | "turn_end";

export interface WorkspaceSnapshotSummary {
  taskId: TaskId;
  turnId: TurnId;
  runId: RunId;
  phase: WorkspaceSnapshotPhase;
  sequence: number;
  capturedAt: string;
  git: GitWorkspaceSnapshotV1 | null;
  changeSummary: ChangeSummary | null;
  relevantFileCount: number;
  dependencyChangeCount: number;
  diagnostic: WorkspaceDiagnosticKind | null;
}

export interface ContextTestResult {
  command: string;
  outcome: string;
  artifactRef: string | null;
}

export interface ContextPointer {
  id: string;
  title: string;
  kind: string;
  locator: string;
  contentHash: string;
  triggerHints: string[];
  byteLength: number;
  sensitivity: string;
}

export interface HistoryFactDelivery {
  sequence: number;
  field: string;
  sha256: string;
  offset: number;
  end: number;
  totalBytes: number;
}

export interface ContextPackageV1 {
  deliveryFacts: HistoryFactDelivery[];
  conversationText: string;
  schemaVersion: 1;
  kind: ContextPackageKind;
  strength: ContextStrength;
  sourceExecutor: ExecutorRef;
  targetExecutor: ExecutorRef;
  objective: string;
  userConstraints: string[];
  acceptanceCriteria: string[];
  completedWork: string[];
  decisions: ContextDecision[];
  pendingWork: string[];
  relevantFiles: RelevantFile[];
  dependencyChanges: RelevantFile[];
  changeSummary: ChangeSummary;
  tests: ContextTestResult[];
  recentTurnRefs: string[];
  contextPointers: ContextPointer[];
  semanticSummary: {
    text: string;
    sourceRunId: RunId;
    providerId: string;
    model: string | null;
    inputSourceSequences: number[];
    outputArtifactRef: string | null;
  } | null;
  provenance: {
    builderVersion: 1;
    builtAt: string;
    redactionRulesetVersion: number;
    redactionCount: number;
    sourceAfterSequence: number;
    sourceThroughSequence: number;
    sourceSequences: number[];
  };
}

export interface BuiltContextPackage {
  package: ContextPackageV1;
  payloadSha256: string;
  byteLength: number;
}

export interface ContextPreviewRequest {
  taskId: TaskId;
  kind: ContextPackageKind;
  strength: ContextStrength;
  sourceExecutor: ExecutorRef;
  targetExecutor: ExecutorRef;
  afterSequence: number;
  throughSequence: number;
}

export interface ContextEditRequest {
  request: ContextPreviewRequest;
  basePayloadSha256: string;
  editedPackage: ContextPackageV1;
}

export interface EventEnvelope {
  schemaVersion: number;
  sequence: number;
  eventId: string;
  stream: { streamType: string; streamId: string };
  streamVersion: number;
  eventType: string;
  occurredAt: string;
  recordedAt: string;
  correlationId: string;
  causationId: string | null;
  source: { kind: string; providerId?: string; sourceSequence?: string | null };
  payload: unknown;
  vendorExtensions: unknown | null;
}

export type EventStreamMessage =
  | { type: "replay_started"; data: { afterSequence: number } }
  | { type: "event"; data: EventEnvelope }
  | { type: "caught_up"; data: { lastSequence: number } }
  | { type: "error"; data: { message: string; retryable: boolean; lastSequence: number } };

export interface CompanionWorkspace {
  lastTerminal?: { state: RunState; occurredAt: string; providerId?: string | null } | null;
  projectId: ProjectId;
  taskId: TaskId;
  displayName: string;
  preferredExecutor: ExecutorRef;
  activePrimaryRun: ActiveRunSummary | null;
  secondaryAgentCount: number;
  reviews?: CompanionReview[];
  pendingInteractionCount: number;
}

export interface CompanionReview {
  runId: RunId;
  providerId: string;
  kind: string;
  status: string;
  createdAt: string;
  finishedAt: string | null;
  completedGroups: number;
  totalGroups: number;
}

export interface CompanionProjection {
  schemaVersion: number;
  revision: number;
  generatedAt: string;
  stale: boolean;
  workspaces: CompanionWorkspace[];
  recentCompletion: {
    taskId: TaskId;
    runId: RunId;
    providerId: string;
    completedAt: string;
  } | null;
}

export interface BootstrapSnapshot {
  projects: ProjectSummary[];
  tasks: TaskSummary[];
  deletedTasks?: TaskSummary[];
  installations: InstallationSummary[];
  activeRuns: ActiveRunSummary[];
  sessionMaps: AgentSessionMapSummary[];
  lastSequence: number;
  companion: CompanionProjection;
}

export interface PendingApprovalOption {
  optionId: string;
  label: string;
  semanticRole: "approve_once" | "reject" | "cancel" | "approve_persistent" | "other";
}

export interface QuestionSchema {
  type?: string; title?: string; description?: string; properties?: Record<string, QuestionSchema>;
  required?: string[]; enum?: (string | number | boolean)[]; enumNames?: string[];
  oneOf?: { const: string; title?: string; description?: string }[];
  anyOf?: { const: string; title?: string; description?: string }[];
  "x-options"?: { const: string; title?: string; description?: string; preview?: string }[];
  "x-allowOther"?: boolean; items?: QuestionSchema; default?: unknown; writeOnly?: boolean;
  minLength?: number; maxLength?: number; minimum?: number; maximum?: number; pattern?: string;
}
export interface QuestionForm { title: string; schema: QuestionSchema }
export interface QuestionAnswer { cancelled: boolean; values: Record<string, unknown> }
export interface PendingApproval {
  question?: QuestionForm;
  approvalId: string;
  turnId: TurnId;
  toolName: string;
  commandSummary: string;
  options: PendingApprovalOption[];
  requestedAtSequence: number;
}

export interface QuotaWindow {
  /** Scoped pools are informational until the active model can be resolved. */
  scope?: string | null;
  id: string;
  windowMinutes: number;
  /** Vendor utilization; null when the agent exposes the window but no usage. */
  usedPercent: number | null;
  resetsAtUnix?: number | null;
}

export interface ContextUsageSnapshot {
  usedTokens: number;
  windowTokens: number;
  cacheHitRatePercent?: number | null;
}

export interface TurnUsage {
  scope: "observed_requests" | "native_turn" | "credits_delta" | "unknown";
  completeness?: "complete" | "partial" | "unknown" | null;
  incompleteReason?: string | null;
  outputTokensPerSecond?: number | null;
  timedRequestCount?: number | null;
  requestCount?: number | null;
  models: string[];
  inputTokens?: number | null;
  outputTokens?: number | null;
  cacheReadTokens?: number | null;
  cacheWriteTokens?: number | null;
  reasoningTokens?: number | null;
  costUsd?: number | null;
  credits?: number | null;
  partial: boolean;
}
export interface UsageRequest {
  generationMs?: number | null;
  requestId: string;
  model?: string | null;
  source: string;
  inputTokens?: number | null;
  outputTokens?: number | null;
  cacheReadTokens?: number | null;
  cacheWriteTokens?: number | null;
  reasoningTokens?: number | null;
  costUsd?: number | null;
  credits?: number | null;
}
export interface RequestUsageReport {
  records: UsageRequest[];
  truncated: boolean;
  unavailableReason?: string | null;
}
export interface SessionUsageSnapshot {
  model?: string | null;
  nativeSummaryId?: string | null;
  requestUsage?: RequestUsageReport | null;
  contextUsedTokens?: number | null;
  contextWindowTokens?: number | null;
  contextUsedPercent?: number | null;
  cacheHitRatePercent?: number | null;
  inputTokens?: number | null;
  outputTokens?: number | null;
  cacheReadTokens?: number | null;
  cacheWriteTokens?: number | null;
  reasoningTokens?: number | null;
  totalTokens?: number | null;
  costUsd?: number | null;
  sessionCostUsd?: number | null;
  credits?: number | null;
  totalsScope?: "session" | "turn" | "request" | "native" | "observed" | "branch" | null;
  observedAtUnixMs: number;
}

export interface QuotaSnapshot {
  sessionUsage?: SessionUsageSnapshot | null;
  windows: QuotaWindow[];
  context?: ContextUsageSnapshot | null;
  collectedAtUnixMs: number;
  staleAfterUnixMs: number;
  planType?: string | null;
  source: string;
  confidence: string;
  parserVersion: string;
}

export interface QuotaSnapshotView {
  providerId: string;
  installationId: string;
  quota: QuotaSnapshot | null;
  diagnostic?: string | null;
}

export interface AgentUpdate {
  installationId: string;
  status: "checking" | "available" | "up_to_date" | "upgrading" | "updated" | "error" | "managed" | "unsupported" | "in_use";
  currentVersion: string | null;
  desktopVersion?: string;
  latestVersion: string | null;
  source: string;
  message: string | null;
  canUpgrade: boolean;
  checkedAt: number;
  inUse?: { token: string; taskCount: number; processCount: number };
}

export type ForkEnvironment = "workspace" | "worktree";
export interface ForkRequest {
  taskId: TaskId;
  turnId: string;
  newTaskId: TaskId;
  environment: ForkEnvironment;
}

export interface ForkReceipt {
  request: ForkRequest; taskAvailable: boolean; completed: boolean;
  status?: "running" | "failed" | "interrupted" | "succeeded";
  stage?: string | null;
  error?: string | null;
}

/** Durable command acceptance only; inspect the run/resource for its final outcome. */
export interface CommandReceipt {
  commandId: string;
  accepted: boolean;
  firstSequence: number | null;
  lastSequence: number | null;
  requestFingerprint?: string;
}

export interface ApprovalSource { turnId: TurnId; requestedAtSequence: number }

export interface TaskCreationIdentity { taskId: TaskId; operationId: string }
export interface TurnSubmission {
  prompt: string;
  model: string | null;
  thinking: string | null;
  permissionMode: string | null;
  attachments: PromptAttachment[];
}
export type CommandInvalidation = { status: "accepted"; acknowledgement: CommandReceipt } | { status: "invalidated" };

/** Frozen send input shared by local clients; submission is only a derived checkpoint. */
export interface SubmissionIntent {
  schemaVersion: 1;
  operationId: string;
  scope: string;
  requestedAt: string;
  task: TaskSummary;
  input: TurnSubmission;
  target: ExecutorRef;
  creation: { operationId: string } | null;
  context: { operationId: string; request: ContextPreviewRequest; hash: string } | null;
  submission: TaskSummary | null;
}
export type SubmissionResolution = { status: "submitted"; taskId: TaskId; sequence: number; turnId: string }
  | { status: "invalidated"; taskId: TaskId }
  | { status: "task_deleted"; taskId: TaskId; deletionSequence: number };
export type SubmissionIntentEntry = { state: "pending"; intent: SubmissionIntent }
  | { state: "closed"; fingerprint: string; resolution: SubmissionResolution; /** Fingerprint of the original start request. */ request?: string };
/** A new send frozen and driven by the service; taskId null creates a new conversation first. */
export interface SubmissionStartRequest {
  operationId: string; scope: string; taskId: TaskId | null; projectId: ProjectId; target: ExecutorRef; input: TurnSubmission;
}
export interface SubmissionIntentPage {
  /** Pending or unreadable identities; Get validates each record independently. */
  operationIds: string[];
  nextCursor: string | null;
  /** Missing only on older local hosts. Never treat true as complete discovery. */
  unrecognizedEntries?: boolean;
}

export interface TaskDeletion { taskId: TaskId; deletionSequence: number }
export interface CoreClient {
  taskDeletion?(taskId: string): Promise<TaskDeletion | null>;
  submissionIntentResolve?(intent: SubmissionIntent): Promise<SubmissionResolution | null>;
  submissionIntentPut?(intent: SubmissionIntent): Promise<SubmissionIntentEntry>;
  submissionIntentGet?(operationId: string, expected?: SubmissionIntent): Promise<SubmissionIntentEntry | null>;
  /** Hints only; initial connect and reconnect require a fresh read. */
  subscribeSubmissionIntents?(refresh: () => void): () => void;
  submissionIntentList?(after?: string | null, limit?: number): Promise<SubmissionIntentPage>;
  submissionIntentClose?(intent: SubmissionIntent): Promise<SubmissionIntentEntry>;
  /** Service-driven send: freezes the request and executes the original steps. */
  submissionStart?(request: SubmissionStartRequest): Promise<SubmissionIntentEntry>;
  /** Continue or fence a pending original send by its identity, from any client. */
  submissionContinue?(operationId: string): Promise<SubmissionIntentEntry>;
  submissionRetire?(operationId: string): Promise<SubmissionIntentEntry>;

  historyPreferences?(): Promise<HistoryPreferencesSnapshot>;
  setHistoryPreferences?(preferences: HistoryPreferences, expectedRevision: number, operationId: string): Promise<void>;
  agentPreferences?(): Promise<AgentPreferencesSnapshot>;
  /** Keep operationId and payload unchanged when recovering a lost acknowledgement. */
  setAgentPreferences?(preferences: AgentPreferences, expectedRevision: number, operationId: string): Promise<void>;
  forkTask?(request: ForkRequest): Promise<TaskId>;
  forkReceipt?(taskId: TaskId, newTaskId: TaskId): Promise<ForkReceipt | null>;
  /** Service-owned "start repair" workflow; any client can discover and continue the original request. */
  reviewRepair?(call: ReviewRepairCall): Promise<ReviewRepairEntry | null>;
  reviewExecutors?(): Promise<ExecutorRef[]>;
  reviewSource?(taskId: TaskId, runId: string, fileId: string): Promise<ReviewSource>;
  turnReviews?(taskId: TaskId): Promise<TurnReviewRun[]>;
  startTurnReview?(request: TurnReviewRequest): Promise<TurnReviewRun>;
  startProjectReview?(request: ProjectReviewRequest): Promise<TurnReviewRun>;
  cancelTurnReview?(taskId: TaskId, runId: string): Promise<void>;

  checkAgentUpdate?(installationId: string, force?: boolean): Promise<AgentUpdate>;
  openAgentUpdater?(installationId: string): Promise<void>;
  upgradeAgent?(installationId: string, confirmationToken?: string): Promise<AgentUpdate>;
  onAgentUpdate?(callback: (update: AgentUpdate) => void): Promise<() => void>;
  /** Re-run native installation and zero-turn readiness probes. */
  scanAgent(providerId: string): Promise<BootstrapSnapshot>;
  prepareSessionRebuild?(taskId: TaskId, expectedVersion: number): Promise<void>;
  queryHistory?(query: HistoryQuery): Promise<HistoryPage>;
  recoverySnapshot(): Promise<RecoverySnapshot>;
  exportDiagnostics(): Promise<"saved" | "cancelled">;
  quotaSnapshots(taskId?: TaskId | null, installationId?: string, force?: boolean): Promise<QuotaSnapshotView[]>;
  modelReadyExecutors?(): Promise<ExecutorRef[]>;
  onQuotaProviderReady?(callback: () => void): Promise<() => void>;
  pendingApprovals(taskId: TaskId, limit?: number): Promise<PendingApproval[]>;
  prepareApprovalResponse?(task: Pick<TaskSummary, "taskId" | "streamVersion">, approvalId: string, optionId: string, source: ApprovalSource, answer: QuestionAnswer | undefined, operationId: string): Promise<ApprovalReceiptQuery>;
  invalidateApprovalResponse?(query: ApprovalReceiptQuery, originalVersion: number): Promise<ApprovalInvalidation>;
  approvalReceipt?(query: ApprovalReceiptQuery): Promise<ApprovalReceipt | null>;
  respondApproval(task: Pick<TaskSummary, "taskId" | "streamVersion">, approvalId: string, optionId: string, source: ApprovalSource, answer?: QuestionAnswer, operationId?: string): Promise<void>;
  taskArtifacts(taskId: TaskId, before?: ArtifactCursor | null, limit?: number): Promise<ArtifactPage>;
  readArtifact(request: ArtifactReadRequest): Promise<ArtifactChunk>;
  exportArtifact(request: ArtifactExportRequest): Promise<"saved" | "cancelled">;
  bootstrap(): Promise<BootstrapSnapshot>;
  timeline(taskId: TaskId): Promise<TimelineItem[]>;
  timelineSnapshot?(taskId: TaskId): Promise<TimelineSnapshot>;
  workspaceSnapshots(taskId: TaskId, limit?: number): Promise<WorkspaceSnapshotSummary[]>;
  buildContextPreview(request: ContextPreviewRequest): Promise<BuiltContextPackage>;
  buildEditedContextPreview(edit: ContextEditRequest): Promise<BuiltContextPackage>;
  confirmContextPackage(
    task: TaskSummary,
    request: ContextPreviewRequest,
    expectedPayloadSha256: string,
    editedPackage: ContextPackageV1 | null,
    operationId?: string,
  ): Promise<void>;
  contextPackageReceipt?(
    task: TaskSummary,
    request: ContextPreviewRequest,
    expectedPayloadSha256: string,
    editedPackage: ContextPackageV1 | null,
    operationId: string,
  ): Promise<CommandReceipt | null>;
  optOutReconciliation(
    task: TaskSummary,
    request: ContextPreviewRequest,
    expectedPayloadSha256: string,
  ): Promise<void>;
  updateProject?(project: ProjectSummary, displayName: string, path: string, additionalDirectories: string[]): Promise<void>;
  registerProject(displayName: string, path: string, additionalDirectories?: string[]): Promise<ProjectId>;
  syncEvents(afterSequence: number, limit?: number): Promise<EventEnvelope[]>;
  deleteTask?(task: TaskSummary): Promise<void>;
  renameTask?(task: TaskSummary, title: string): Promise<void>;
  uploadAttachment?(request: AttachmentUploadRequest, payloadBase64: string): Promise<PromptAttachment>;
  attachmentUploadReceipt?(request: AttachmentUploadRequest): Promise<PromptAttachment | null>;
  createTask(projectId: ProjectId, title: string, executor: ExecutorRef, identity?: TaskCreationIdentity): Promise<TaskId>;
  createTaskReceipt?(projectId: ProjectId, title: string, executor: ExecutorRef, identity: TaskCreationIdentity): Promise<CommandReceipt | null>;
  invalidateTaskCreation?(projectId: ProjectId, title: string, executor: ExecutorRef, identity: TaskCreationIdentity): Promise<CommandInvalidation>;
  submitTurnReceipt?(task: TaskSummary, input: TurnSubmission, operationId: string): Promise<CommandReceipt | null>;
  invalidateTurnSubmission?(task: TaskSummary, input: TurnSubmission, operationId: string): Promise<CommandInvalidation>;
  invalidateContextPackage?(task: TaskSummary, request: ContextPreviewRequest, expectedPayloadSha256: string, editedPackage: ContextPackageV1 | null, operationId: string): Promise<CommandInvalidation>;
  setPreferredExecutor(task: TaskSummary, executor: ExecutorRef): Promise<void>;
  captureWorkspaceDiff(task: TaskSummary): Promise<void>;
  permissionCatalog?(executor: ExecutorRef, taskId?: TaskId): Promise<PermissionCatalog>;
  /** Complete selection for the next turn; null model/thinking uses native defaults. */
  submitTurn(task: TaskSummary, prompt: string, model?: string | null, thinking?: string | null, permissionMode?: string | null, attachments?: PromptAttachment[], operationId?: string): Promise<void>;
  setRunningPermissionMode?(taskId: TaskId, runId: RunId, executor: ExecutorRef, mode: string): Promise<void>;
  availableModels?(executor: ExecutorRef, selectedModel?: string | null, taskId?: TaskId): Promise<ModelCatalog>;
  cancelTurn(task: TaskSummary, operationId?: string): Promise<void>;
  cancelTurnReceipt?(task: TaskSummary, operationId: string): Promise<CommandReceipt | null>;
  subscribe(afterSequence: number, listener: (message: EventStreamMessage) => void): Promise<() => void>;
}

export type RecoveryCode =
  | "installation_unconfigured" | "installation_broken" | "installation_unsupported"
  | "installation_not_ready" | "driver_unavailable" | "run_ownership_unknown"
  | "run_interrupting" | "approval_pending" | "session_unavailable";

export interface RecoveryItem {
  code: RecoveryCode;
  providerId: string | null;
  taskId: TaskId | null;
  runId: string | null;
  turnId: string | null;
  expectedStreamVersion: number | null;
  canCancel: boolean;
}

export interface RecoverySnapshot {
  schemaVersion: number;
  generatedAt: string;
  lastSequence: number;
  truncated: boolean;
  items: RecoveryItem[];
}

export interface ThinkingOption { id: string; label: string }
export interface ModelOption {
  id: string;
  label: string;
  thinkingOptions: ThinkingOption[];
  defaultThinking: string | null;
}
export interface ModelCatalog { models: ModelOption[]; defaultModel: string | null }

export interface PermissionMode { id: string; label: string; description: string; dangerous: boolean; }
export interface PermissionCatalog { modes: PermissionMode[]; defaultModeId: string | null; }

export interface HistoryQuery { taskId: TaskId; afterSequence: number; throughSequence?: number | null; limit: number; kind: "turns" | "search" | "decisions" | "changes"; text: string }
export interface HistoryPage { items: {sequence: number; turnId: string | null; kind: string; text: string; occurredAt: string; attachments?: PromptAttachment[]}[]; nextCursor: number | null; throughSequence: number; coverage: string }

export type ReviewKind = "code" | "security" | "performance";
// Retired kinds remain readable in persisted review results.
export type HistoricalReviewKind = ReviewKind | "tests" | "requirements" | "impact";
export type ReviewStatus = "queued" | "running" | "cancelling" | "completed" | "failed" | "cancelled" | "interrupted";
export interface ReviewFixRef { runId: string; inputSha256: string; findingIndices: number[] }
export interface ReviewRepairIntent {
  operationId: string; taskId: TaskId; baseVersion: number; reference: ReviewFixRef; target: ExecutorRef;
  context: { operationId: string; request: ContextPreviewRequest; hash: string } | null; createdAt: string;
}
export type ReviewRepairState =
  | { state: "pending"; submissionVersion: number | null; contextConfirmed: boolean; lastError: string | null }
  | { state: "submitted"; acknowledgement: CommandReceipt }
  | { state: "abandoned" };
export type ReviewRepairEntry = { intent: ReviewRepairIntent; updatedAt: string } & ReviewRepairState;
export type ReviewRepairCall =
  | { action: "start"; operationId: string; taskId: TaskId; reference: ReviewFixRef; target: ExecutorRef | null }
  | { action: "get"; taskId: TaskId; runId: string }
  | { action: "resume" | "abandon"; taskId: TaskId; runId: string; operationId: string };
export interface ReviewRecheck { runId: string; repairTurnId: string }
export interface ReviewRepair { turnId: string; findingIndices: number[]; status: string; recheckRuns: string[] }
export interface TurnReviewRequest {
  recheckOf?: ReviewRecheck | null;
  taskId: TaskId; turnId: string; runId: string; executor: ExecutorRef; kind: ReviewKind; retryOf: string | null;
}
export interface ProjectReviewRequest extends Omit<TurnReviewRequest, "turnId"> {
  turnId: null; sourceSnapshot: string;
}
export interface ReviewActivity { itemId: string; data: Record<string, unknown>; legacy?: boolean }
export interface TurnReviewRun {
  activityRevision?: number;
  activities?: ReviewActivity[];
  workspaceReview?: boolean;
  repairs?: ReviewRepair[];
  request: Omit<TurnReviewRequest, "kind" | "turnId"> & { kind: HistoricalReviewKind; turnId: string | null; sourceSnapshot?: string | null }; status: ReviewStatus; createdAt: string; finishedAt: string | null;
  nativeSessionId: string | null; sourceSequence: number; sourceArtifact: ArtifactMetadata | null;
  inputSha256: string; coverage: string; result: string; error: string | null; report?: ReviewReport | null;
  workflow?: {contextScope: string; groups: {id: string; fileIds: string[]; status: string; sessions: string[]; contextRequests: number; contextLog?: {phase: string; requests: unknown[]; responseSha256: string}[]; initialOutput: string; verificationOutput: string; error: string | null}[]} | null;
}

export interface ReviewSource { fileId: string; rootId: string; rootLabel?: string; path: string; before: string | null; after: string | null }
export interface SecurityCitation {
  sourceId: string; side: "before" | "after"; startLine: number; endLine: number; evidence: string;
  path?: string | null; rootLabel?: string | null;
}
export interface SecurityEvidence {
  attackClass: string; attacker: string; boundary: string; preconditions: string; controls: string; impact: string;
  trace: (SecurityCitation & {role: string; explanation: string})[]; blockers: string[]; validationPlan: string;
}
export interface SecurityAssessment {
  evidence: SecurityEvidence | null; evidenceStatus: "source_checked" | "needs_validation";
  runtimeValidation: "not_run"; notes: string[];
}
export interface SecurityCheck {
  attackClass: string; label: string; status: "reviewed" | "not_applicable" | "needs_context" | "not_reported";
  reason: string; citations: SecurityCitation[];
}
export interface SecurityReview {
  boundaryAnalysis: string; controlAnalysis: string; impactAnalysis: string; unresolved: string[];
}
export interface ReviewFinding {
  fileId: string; severity: "critical" | "high" | "medium" | "low";
  title: string; description: string; suggestion: string; side: "before" | "after";
  startLine: number; endLine: number; evidence: string;
  verification?: {verdict: "confirmed" | "refuted" | "uncertain"; reason: string; citations?: {sourceId: string; path?: string; rootLabel?: string; side: "before" | "after"; startLine: number; endLine: number; evidence: string}[]; securityReview?: SecurityReview | null; securityEvidence?: SecurityAssessment | null} | null;
  security?: SecurityAssessment | null;
  path: string | null; rootId: string | null; rootLabel?: string | null; locationVerified: boolean; locationNote: string;
}
export interface ReviewReport {
  schemaVersion: number; structured: boolean; findings: ReviewFinding[];
  files: {fileId: string; rootId: string; rootLabel?: string; path: string; status: "reported" | "partial" | "not_reported" | "unavailable"; rules: string[]; securityChecks?: SecurityCheck[]}[];
  warnings: string[];
}

/** Persistable recovery identity; contains neither the answer nor the chosen option. */
export interface ApprovalReceiptQuery extends ApprovalSource {
  taskId: TaskId;
  approvalId: string;
  operationId: string;
  requestFingerprint: string;
}
/** Driver delivery is distinct from completion of the approved action. */
export interface ApprovalReceipt {
  acknowledgement: CommandReceipt;
  delivery: "unknown" | "sent" | "failed";
}

export type ApprovalInvalidation = { status: "accepted"; receipt: ApprovalReceipt } | { status: "invalidated" };
