import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ulid } from "ulid";
import type {
  ReviewSource, ProjectReviewRequest, TurnReviewRun, TurnReviewRequest, AgentUpdate, ArtifactChunk, ArtifactCursor, ArtifactExportRequest, ArtifactPage, ArtifactReadRequest,
  BootstrapSnapshot, PendingApproval, QuotaSnapshotView, ModelCatalog,
  BuiltContextPackage,
  ContextEditRequest,
  ContextPackageV1,
  ContextPreviewRequest,
  CoreClient, HistoryQuery, HistoryPage,
  EventEnvelope,
  EventStreamMessage,
  ExecutorRef,
  ProjectId,
  ProjectSummary,
  TaskId,
  TaskSummary,
  TimelineItem,
  WorkspaceSnapshotSummary,
} from "@dogcodes/protocol";

import { createInvalidationSubscriber } from "./invalidationWatch";
import { translate as t } from "./i18n";
const watchSubmissions = createInvalidationSubscriber("core_submission_watch", "core_submission_unwatch");

type Command = { type: string; data: Record<string, unknown> };

function taskCreationCommand(projectId: ProjectId, title: string, executor: ExecutorRef, taskId: TaskId): Command {
  return { type: "create_task", data: { taskId, projectId, title, preferredExecutor: executor } };
}
function turnSubmissionCommand(task: TaskSummary, input: import("@dogcodes/protocol").TurnSubmission): Command {
  return { type: "submit_turn", data: { taskId: task.taskId, ...input, executorOverride: task.preferredExecutor } };
}
function contextPackageCommand(request: ContextPreviewRequest, expectedPayloadSha256: string, editedPackage: ContextPackageV1 | null): Command {
  return { type: "confirm_context_package", data: { request, expectedPayloadSha256, editedPackage } };
}
function approvalCommand(task: Pick<TaskSummary, "taskId" | "streamVersion">, approvalId: string, optionId: string, source: import("@dogcodes/protocol").ApprovalSource, answer?: import("@dogcodes/protocol").QuestionAnswer): Command {
  return { type: "respond_to_approval", data: { taskId: task.taskId, approvalId, optionId, turnId: source.turnId, requestedAtSequence: source.requestedAtSequence, ...(answer ? { answer } : {}) } };
}
function cancelTurnCommand(task: TaskSummary): Command {
  const active = task.activePrimaryRun;
  if (!active) throw new Error(t("tauriClient.noRun"));
  return { type: "cancel_turn", data: { taskId: task.taskId, turnId: active.turnId, runId: active.runId } };
}
function commandEnvelope(payload: Command, expectedStreamVersion: number | null, operationId: string) {
  return {
    schemaVersion: 1, commandId: operationId, idempotencyKey: operationId,
    actor: { kind: "local_user" }, expectedStreamVersion,
    issuedAt: new Date().toISOString(), payload,
  };
}

const typedId = <T extends string>(prefix: string) => `${prefix}_${ulid()}` as T;

export class TauriCoreClient implements CoreClient {
  taskDeletion(taskId: string): Promise<import("@dogcodes/protocol").TaskDeletion | null> {
    return invoke("core_task_deletion", { taskId });
  }
  subscribeSubmissionIntents(refresh: () => void): () => void {
    return watchSubmissions("submissions", {}, refresh);
  }
  submissionIntentResolve(intent: import("@dogcodes/protocol").SubmissionIntent): Promise<import("@dogcodes/protocol").SubmissionResolution | null> {
    return invoke("core_submission_intents", { call: { type: "resolve", intent } });
  }
  submissionIntentPut(intent: import("@dogcodes/protocol").SubmissionIntent): Promise<import("@dogcodes/protocol").SubmissionIntentEntry> {
    return invoke("core_submission_intents", { call: { type: "put", intent } });
  }
  submissionIntentGet(operationId: string, expected?: import("@dogcodes/protocol").SubmissionIntent, expectedStart?: import("@dogcodes/protocol").SubmissionStartRequest): Promise<import("@dogcodes/protocol").SubmissionIntentEntry | null> {
    if (expectedStart) return invoke("core_submission_intents", { call: { type: "get_start", operationId, request: expectedStart } });
    return invoke("core_submission_intents", { call: { type: "get", operationId, expected: expected ?? null } });
  }
  submissionIntentList(after: string | null = null, limit = 50): Promise<import("@dogcodes/protocol").SubmissionIntentPage> {
    return invoke("core_submission_intents", { call: { type: "list", after, limit } });
  }
  submissionIntentClose(intent: import("@dogcodes/protocol").SubmissionIntent): Promise<import("@dogcodes/protocol").SubmissionIntentEntry> {
    return invoke("core_submission_intents", { call: { type: "close", intent } });
  }
  submissionStart(request: import("@dogcodes/protocol").SubmissionStartRequest): Promise<import("@dogcodes/protocol").SubmissionIntentEntry> {
    return invoke("core_submission_intents", { call: { type: "start", ...request } });
  }
  submissionContinue(operationId: string): Promise<import("@dogcodes/protocol").SubmissionIntentEntry> {
    return invoke("core_submission_intents", { call: { type: "continue", operationId } });
  }
  submissionRetire(operationId: string): Promise<import("@dogcodes/protocol").SubmissionIntentEntry> {
    return invoke("core_submission_intents", { call: { type: "retire", operationId } });
  }
  historyPreferences(): Promise<import("@dogcodes/protocol").HistoryPreferencesSnapshot> { return invoke("core_history_preferences"); }
  async setHistoryPreferences(preferences: import("@dogcodes/protocol").HistoryPreferences, expectedRevision: number, operationId: string): Promise<void> {
    await this.execute({ type: "set_history_preferences", data: { ...preferences } }, expectedRevision, operationId);
  }
  agentPreferences(): Promise<import("@dogcodes/protocol").AgentPreferencesSnapshot> {
    return invoke("core_agent_preferences");
  }
  async setAgentPreferences(preferences: import("@dogcodes/protocol").AgentPreferences, expectedRevision: number, operationId: string): Promise<void> {
    await this.execute({ type: "set_agent_preferences", data: { ...preferences } }, expectedRevision, operationId);
  }
  reviewRepair(call: import("@dogcodes/protocol").ReviewRepairCall): Promise<import("@dogcodes/protocol").ReviewRepairEntry | null> {
    return invoke("core_review_repair", { call });
  }
  reviewExecutors(): Promise<ExecutorRef[]> { return invoke("core_review_executors"); }
  reviewSource(taskId: TaskId, runId: string, fileId: string): Promise<ReviewSource> { return invoke("core_review_source", { taskId, runId, fileId }); }
  forkReceipt(taskId: TaskId, newTaskId: TaskId): Promise<import("@dogcodes/protocol").ForkReceipt | null> { return invoke("core_fork_receipt", { taskId, newTaskId }); }
  forkTask(request: import("@dogcodes/protocol").ForkRequest): Promise<TaskId> { return invoke("core_fork_task", { request }); }
  turnReviews(taskId: TaskId): Promise<TurnReviewRun[]> { return invoke("core_turn_reviews", { taskId }); }
  startProjectReview(request: ProjectReviewRequest): Promise<TurnReviewRun> { return invoke("core_start_project_review", { request }); }
  startTurnReview(request: TurnReviewRequest): Promise<TurnReviewRun> { return invoke("core_start_turn_review", { request }); }
  cancelTurnReview(taskId: TaskId, runId: string): Promise<void> { return invoke("core_cancel_turn_review", { taskId, runId }); }

  checkAgentUpdate(installationId: string, force = false): Promise<AgentUpdate> { return invoke("core_check_agent_update", { installationId, force }); }
  openAgentUpdater(installationId: string): Promise<void> { return invoke("core_open_agent_updater", { installationId }); }
  upgradeAgent(installationId: string, confirmationToken?: string): Promise<AgentUpdate> { return invoke("core_upgrade_agent", { installationId, confirmationToken }); }
  onAgentUpdate(callback: (update: AgentUpdate) => void): Promise<() => void> { return listen<AgentUpdate>("agent-update-changed", event => callback(event.payload)); }
  scanAgent(providerId: string): Promise<BootstrapSnapshot> { return invoke("core_scan_agent", { providerId }); }
  prepareSessionRebuild(taskId: TaskId, expectedVersion: number): Promise<void> { return invoke("core_prepare_session_rebuild", {taskId, expectedVersion}); }
  queryHistory(query: HistoryQuery): Promise<HistoryPage> { return invoke("core_query_history", { query }); }
  taskArtifacts(taskId: TaskId, before: ArtifactCursor | null = null, limit = 20): Promise<ArtifactPage> {
    return invoke("core_task_artifacts", { taskId, before, limit });
  }

  readArtifact(request: ArtifactReadRequest): Promise<ArtifactChunk> {
    return invoke("core_read_artifact", { request });
  }

  exportArtifact(request: ArtifactExportRequest): Promise<"saved" | "cancelled"> {
    return invoke("core_export_artifact", { request });
  }

  bootstrap(): Promise<BootstrapSnapshot> {
    return invoke("core_bootstrap");
  }

  timeline(taskId: TaskId): Promise<TimelineItem[]> {
    return invoke("core_timeline", { taskId });
  }

  timelineSnapshot(taskId: TaskId): Promise<import("@dogcodes/protocol").TimelineSnapshot> {
    return invoke("core_timeline_snapshot", { taskId });
  }

  workspaceSnapshots(taskId: TaskId, limit = 20): Promise<WorkspaceSnapshotSummary[]> {
    return invoke("core_workspace_snapshots", { taskId, limit });
  }

  buildContextPreview(request: ContextPreviewRequest): Promise<BuiltContextPackage> {
    return invoke("core_build_context_preview", { request });
  }

  buildEditedContextPreview(edit: ContextEditRequest): Promise<BuiltContextPackage> {
    return invoke("core_build_edited_context_preview", { edit });
  }

  async confirmContextPackage(
    task: TaskSummary,
    request: ContextPreviewRequest,
    expectedPayloadSha256: string,
    editedPackage: ContextPackageV1 | null,
    operationId?: string,
  ): Promise<void> {
    await this.execute(
      contextPackageCommand(request, expectedPayloadSha256, editedPackage),
      task.streamVersion,
      operationId,
    );
  }

  contextPackageReceipt(task: TaskSummary, request: ContextPreviewRequest, expectedPayloadSha256: string, editedPackage: ContextPackageV1 | null, operationId: string): Promise<import("@dogcodes/protocol").CommandReceipt | null> {
    return invoke("core_command_receipt", { envelope: commandEnvelope(contextPackageCommand(request, expectedPayloadSha256, editedPackage), task.streamVersion, operationId) });
  }

  async optOutReconciliation(
    task: TaskSummary,
    request: ContextPreviewRequest,
    expectedPayloadSha256: string,
  ): Promise<void> {
    await this.execute(
      {
        type: "opt_out_reconciliation",
        data: { request, expectedPayloadSha256 },
      },
      task.streamVersion,
    );
  }

  syncEvents(afterSequence: number, limit = 10_000): Promise<EventEnvelope[]> {
    return invoke("core_sync_events", { afterSequence, limit });
  }

  async updateProject(project: ProjectSummary, displayName: string, path: string, additionalDirectories: string[]): Promise<void> {
    if (project.streamVersion === undefined) throw new Error(t("tauriClient.projectStale"));
    await invoke("core_update_local_project", { projectId: project.projectId, expectedStreamVersion: project.streamVersion, displayName, path, additionalDirectories });
  }

  async registerProject(displayName: string, path: string, additionalDirectories?: string[]): Promise<ProjectId> {
    return invoke("core_register_local_project", { displayName, path, ...(additionalDirectories ? { additionalDirectories } : {}) });
  }

  uploadAttachment(request: import("@dogcodes/protocol").AttachmentUploadRequest, payloadBase64: string): Promise<import("@dogcodes/protocol").PromptAttachment> {
    return invoke("core_save_attachment", { request, payloadBase64 });
  }
  attachmentUploadReceipt(request: import("@dogcodes/protocol").AttachmentUploadRequest): Promise<import("@dogcodes/protocol").PromptAttachment | null> {
    return invoke("core_attachment_upload_receipt", { request });
  }
  async createTask(projectId: ProjectId, title: string, executor: ExecutorRef, identity?: import("@dogcodes/protocol").TaskCreationIdentity): Promise<TaskId> {
    const taskId = identity?.taskId ?? typedId<TaskId>("tsk");
    await this.execute(taskCreationCommand(projectId, title, executor, taskId), 0, identity?.operationId);
    return taskId;
  }
  createTaskReceipt(projectId: ProjectId, title: string, executor: ExecutorRef, identity: import("@dogcodes/protocol").TaskCreationIdentity): Promise<import("@dogcodes/protocol").CommandReceipt | null> {
    return invoke("core_command_receipt", { envelope: commandEnvelope(taskCreationCommand(projectId, title, executor, identity.taskId), 0, identity.operationId) });
  }
  invalidateTaskCreation(projectId: ProjectId, title: string, executor: ExecutorRef, identity: import("@dogcodes/protocol").TaskCreationIdentity): Promise<import("@dogcodes/protocol").CommandInvalidation> {
    return invoke("core_invalidate_pending_command", { envelope: commandEnvelope(taskCreationCommand(projectId, title, executor, identity.taskId), 0, identity.operationId) });
  }
  submitTurnReceipt(task: TaskSummary, input: import("@dogcodes/protocol").TurnSubmission, operationId: string): Promise<import("@dogcodes/protocol").CommandReceipt | null> {
    return invoke("core_command_receipt", { envelope: commandEnvelope(turnSubmissionCommand(task, input), task.streamVersion, operationId) });
  }
  invalidateTurnSubmission(task: TaskSummary, input: import("@dogcodes/protocol").TurnSubmission, operationId: string): Promise<import("@dogcodes/protocol").CommandInvalidation> {
    return invoke("core_invalidate_pending_command", { envelope: commandEnvelope(turnSubmissionCommand(task, input), task.streamVersion, operationId) });
  }
  invalidateContextPackage(task: TaskSummary, request: ContextPreviewRequest, expectedPayloadSha256: string, editedPackage: ContextPackageV1 | null, operationId: string): Promise<import("@dogcodes/protocol").CommandInvalidation> {
    return invoke("core_invalidate_pending_command", { envelope: commandEnvelope(contextPackageCommand(request, expectedPayloadSha256, editedPackage), task.streamVersion, operationId) });
  }

  async deleteTask(task: TaskSummary): Promise<void> {
    await this.execute({ type: "delete_task", data: { taskId: task.taskId } }, task.streamVersion);
  }



  async renameTask(task: TaskSummary, title: string): Promise<void> {
    await this.execute({ type: "rename_task", data: { taskId: task.taskId, title } }, task.streamVersion);
  }

  async setPreferredExecutor(task: TaskSummary, executor: ExecutorRef): Promise<void> {
    await this.execute(
      {
        type: "set_preferred_executor",
        data: {
          taskId: task.taskId,
          executor,
        },
      },
      task.streamVersion,
    );
  }

  async captureWorkspaceDiff(task: TaskSummary): Promise<void> {
    await this.execute(
      {
        type: "capture_workspace_diff",
        data: { taskId: task.taskId },
      },
      task.streamVersion,
    );
  }

  quotaSnapshots(taskId?: TaskId | null, installationId?: string, force = false): Promise<QuotaSnapshotView[]> {
    return invoke("core_quota_snapshots", { taskId: taskId ?? null, installationId: installationId ?? null, force });
  }

  modelReadyExecutors(): Promise<import("@dogcodes/protocol").ExecutorRef[]> {
    return invoke("core_model_ready_executors");
  }

  onQuotaProviderReady(callback: () => void): Promise<() => void> {
    return listen("quota-provider-ready", callback);
  }

  recoverySnapshot(): Promise<import("@dogcodes/protocol").RecoverySnapshot> {
    return invoke("core_recovery_snapshot");
  }

  exportDiagnostics(): Promise<"saved" | "cancelled"> {
    return invoke("core_export_diagnostics");
  }

  pendingApprovals(taskId: TaskId, limit = 20): Promise<PendingApproval[]> {
    return invoke("core_pending_approvals", { taskId, limit });
  }

  prepareApprovalResponse(task: Pick<TaskSummary, "taskId" | "streamVersion">, approvalId: string, optionId: string, source: import("@dogcodes/protocol").ApprovalSource, answer: import("@dogcodes/protocol").QuestionAnswer | undefined, operationId: string): Promise<import("@dogcodes/protocol").ApprovalReceiptQuery> {
    return invoke("core_prepare_approval_response", { envelope: commandEnvelope(approvalCommand(task, approvalId, optionId, source, answer), task.streamVersion, operationId) });
  }

  invalidateApprovalResponse(query: import("@dogcodes/protocol").ApprovalReceiptQuery, originalVersion: number): Promise<import("@dogcodes/protocol").ApprovalInvalidation> {
    return invoke("core_invalidate_approval_response", { query, originalVersion });
  }

  approvalReceipt(query: import("@dogcodes/protocol").ApprovalReceiptQuery): Promise<import("@dogcodes/protocol").ApprovalReceipt | null> {
    return invoke("core_approval_receipt", { query });
  }

  async respondApproval(
    task: Pick<TaskSummary, "taskId" | "streamVersion">,
    approvalId: string,
    optionId: string,
    source: import("@dogcodes/protocol").ApprovalSource,
    answer?: import("@dogcodes/protocol").QuestionAnswer,
    operationId?: string,
  ): Promise<void> {
    await this.execute(
      approvalCommand(task, approvalId, optionId, source, answer),
      task.streamVersion,
      operationId,
    );
  }

  availableModels(executor: ExecutorRef, selectedModel: string | null = null, taskId?: TaskId): Promise<ModelCatalog> {
    return invoke("core_available_models", { executor, selectedModel, taskId: taskId ?? null });
  }

  permissionCatalog(executor: ExecutorRef, taskId?: TaskId): Promise<import("@dogcodes/protocol").PermissionCatalog> {
    return invoke("core_permission_catalog", {executor, taskId: taskId ?? null});
  }

  setRunningPermissionMode(taskId: TaskId, runId: import("@dogcodes/protocol").RunId, executor: ExecutorRef, mode: string): Promise<void> {
    return invoke("core_set_running_permission_mode", {taskId, runId, executor, mode});
  }

  async submitTurn(task: TaskSummary, prompt: string, model: string | null = null, thinking: string | null = null, permissionMode: string | null = null, attachments: import("@dogcodes/protocol").PromptAttachment[] = [], operationId?: string): Promise<void> {
    await this.execute(turnSubmissionCommand(task, { prompt, model, thinking, permissionMode, attachments }), task.streamVersion, operationId);
  }

  async cancelTurn(task: TaskSummary, operationId?: string): Promise<void> {
    await this.execute(cancelTurnCommand(task), task.streamVersion, operationId);
  }

  cancelTurnReceipt(task: TaskSummary, operationId: string): Promise<import("@dogcodes/protocol").CommandReceipt | null> {
    return invoke("core_command_receipt", { envelope: commandEnvelope(cancelTurnCommand(task), task.streamVersion, operationId) });
  }

  async subscribe(
    afterSequence: number,
    listener: (message: EventStreamMessage) => void,
  ): Promise<() => void> {
    const subscriptionId = `sub_${ulid()}`;
    const channel = new Channel<EventStreamMessage>();
    channel.onmessage = listener;
    await invoke("core_subscribe", {
      afterSequence,
      subscriptionId,
      onEvent: channel,
    });
    return () => {
      channel.onmessage = () => undefined;
      void invoke("core_unsubscribe", { subscriptionId });
    };
  }

  private execute(payload: Command, expectedStreamVersion: number | null = null, operationId?: string) {
    const commandId = operationId ?? typedId<`cmd_${string}`>("cmd");
    return invoke("core_execute", {
      envelope: commandEnvelope(payload, expectedStreamVersion, commandId),
    });
  }
}
