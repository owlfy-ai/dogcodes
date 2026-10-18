import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { CommandReceipt, ContextPreviewRequest, ReviewFixRef, TaskSummary } from "@dogcodes/protocol";
import { TauriCoreClient } from "./tauriClient";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: vi.fn() }));
const task: TaskSummary = {
  taskId: "tsk_original", projectId: "prj_original", title: "Original",
  preferredExecutor: { providerId: "codex", installationId: null }, activePrimaryRun: null, streamVersion: 7,
};
const reference: ReviewFixRef = { runId: "run_review", inputSha256: "frozen-hash", findingIndices: [0, 3] };
const request: ContextPreviewRequest = {
  taskId: task.taskId, kind: "handoff", strength: "standard", sourceExecutor: task.preferredExecutor,
  targetExecutor: { providerId: "claude", installationId: "ins_target" }, afterSequence: 0, throughSequence: 20,
};
const receipt: CommandReceipt = { commandId: "cmd_original", accepted: true, firstSequence: 21, lastSequence: 22, requestFingerprint: "v1:hash" };
beforeEach(() => vi.resetAllMocks());

it("sends repair requests to the service workflow without building commands in the client", async () => {
  const client = new TauriCoreClient();
  vi.mocked(invoke).mockResolvedValue(null);
  const call = { action: "start", operationId: "cmd_original", taskId: task.taskId, reference, target: null } as const;
  await client.reviewRepair(call);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_repair", { call });
});

it("uses a fixed context command identity and preserves the hash, target and original version in lookup", async () => {
  const client = new TauriCoreClient();
  vi.mocked(invoke).mockResolvedValue(receipt);
  await client.confirmContextPackage(task, request, "original-package-hash", null, "cmd_context");
  const sent = vi.mocked(invoke).mock.calls[0][1] as { envelope: Record<string, unknown> };
  expect(await client.contextPackageReceipt(task, request, "original-package-hash", null, "cmd_context")).toEqual(receipt);
  expect(invoke).toHaveBeenLastCalledWith("core_command_receipt", { envelope: { ...sent.envelope, issuedAt: expect.any(String) } });
  expect(sent).toMatchObject({ envelope: { expectedStreamVersion: 7, commandId: "cmd_context", idempotencyKey: "cmd_context",
    payload: { type: "confirm_context_package", data: { request, expectedPayloadSha256: "original-package-hash", editedPackage: null } } } });
});

it("keeps missing acceptance distinct and never retries a mutation on missing or failed lookup", async () => {
  const client = new TauriCoreClient();
  vi.mocked(invoke).mockResolvedValueOnce(null).mockRejectedValueOnce(new Error("different request"));
  expect(await client.contextPackageReceipt(task, request, "hash", null, "cmd_context")).toBeNull();
  await expect(client.contextPackageReceipt(task, request, "hash", null, "cmd_context")).rejects.toThrow("different request");
  expect(vi.mocked(invoke).mock.calls.map(call => call[0])).toEqual(["core_command_receipt", "core_command_receipt"]);
});

it("uses the same explicit run and version for cancellation and read-only recovery", async () => {
  const client = new TauriCoreClient();
  const original: TaskSummary = { ...task, activePrimaryRun: { runId: "run_old", turnId: "trn_old", executor: task.preferredExecutor, state: "running", startedAt: "now" } };
  vi.mocked(invoke).mockResolvedValue(receipt);
  await client.cancelTurn(original, "cmd_stop");
  const sent = vi.mocked(invoke).mock.calls[0][1] as { envelope: Record<string, unknown> };
  expect(await client.cancelTurnReceipt(original, "cmd_stop")).toEqual(receipt);
  expect(invoke).toHaveBeenLastCalledWith("core_command_receipt", { envelope: { ...sent.envelope, issuedAt: expect.any(String) } });
  expect(sent).toMatchObject({ envelope: { idempotencyKey: "cmd_stop", expectedStreamVersion: 7,
    payload: { type: "cancel_turn", data: { taskId: task.taskId, runId: "run_old", turnId: "trn_old" } } } });
});

it("prepares the exact approval command and queries without retaining its answer", async () => {
  const client = new TauriCoreClient();
  const source = { turnId: "trn_old" as const, requestedAtSequence: 9 };
  const answer = { cancelled: false, values: { password: "sensitive-answer" } };
  const query = { taskId: task.taskId, ...source, approvalId: "reused-id", operationId: "cmd_approval", requestFingerprint: "v1:hash" };
  vi.mocked(invoke).mockResolvedValueOnce(query).mockResolvedValueOnce(receipt).mockResolvedValueOnce({ acknowledgement: receipt, delivery: "unknown" });
  expect(await client.prepareApprovalResponse(task, "reused-id", "", source, answer, "cmd_approval")).toEqual(query);
  const prepared = vi.mocked(invoke).mock.calls[0][1] as { envelope: Record<string, unknown> };
  expect(vi.mocked(invoke).mock.calls[0][0]).toBe("core_prepare_approval_response");
  await client.respondApproval(task, "reused-id", "", source, answer, "cmd_approval");
  expect(invoke).toHaveBeenLastCalledWith("core_execute", { envelope: { ...prepared.envelope, issuedAt: expect.any(String) } });
  expect(await client.approvalReceipt(query)).toEqual({ acknowledgement: receipt, delivery: "unknown" });
  expect(invoke).toHaveBeenLastCalledWith("core_approval_receipt", { query });
  expect(JSON.stringify(vi.mocked(invoke).mock.calls[2])).not.toContain("sensitive-answer");
  expect(invoke).toHaveBeenCalledTimes(3);
});

it("keeps explicit approval invalidation separate from read-only lookup and decision submission", async () => {
  const client = new TauriCoreClient();
  const query = { taskId: task.taskId, turnId: "trn_old" as const, requestedAtSequence: 9, approvalId: "native-id", operationId: "cmd_approval", requestFingerprint: "v1:hash" };
  vi.mocked(invoke).mockResolvedValue({ status: "invalidated" });
  expect(await client.invalidateApprovalResponse(query, 7)).toEqual({ status: "invalidated" });
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_invalidate_approval_response", { query, originalVersion: 7 });
});

it("binds task creation, lookup and retirement to the same task ID, payload and zero version", async () => {
  const client = new TauriCoreClient();
  const identity = { taskId: "tsk_frozen_new" as const, operationId: "cmd_create" };
  vi.mocked(invoke).mockResolvedValue(receipt);
  expect(await client.createTask(task.projectId, "Original title", task.preferredExecutor, identity)).toBe(identity.taskId);
  const sent = vi.mocked(invoke).mock.calls[0][1] as { envelope: Record<string, unknown> };
  await client.createTaskReceipt(task.projectId, "Original title", task.preferredExecutor, identity);
  expect(invoke).toHaveBeenLastCalledWith("core_command_receipt", { envelope: { ...sent.envelope, issuedAt: expect.any(String) } });
  await client.invalidateTaskCreation(task.projectId, "Original title", task.preferredExecutor, identity);
  expect(invoke).toHaveBeenLastCalledWith("core_invalidate_pending_command", { envelope: { ...sent.envelope, issuedAt: expect.any(String) } });
  expect(sent).toMatchObject({ envelope: { commandId: identity.operationId, idempotencyKey: identity.operationId, expectedStreamVersion: 0,
    payload: { type: "create_task", data: { taskId: identity.taskId, projectId: task.projectId, title: "Original title", preferredExecutor: task.preferredExecutor } } } });
  expect(invoke).toHaveBeenCalledTimes(3);
});
it("keeps every turn selection and attachment in the exact submission identity", async () => {
  const client = new TauriCoreClient();
  const input = { prompt: "Original prompt", model: "model-choice", thinking: "high", permissionMode: "default",
    attachments: [{ name: "original.png", path: "/private/attachments/original.png", mimeType: "image/png" }] };
  vi.mocked(invoke).mockResolvedValue(receipt);
  await client.submitTurn(task, input.prompt, input.model, input.thinking, input.permissionMode, input.attachments, "cmd_send");
  const sent = vi.mocked(invoke).mock.calls[0][1] as { envelope: Record<string, unknown> };
  await client.submitTurnReceipt(task, input, "cmd_send");
  expect(invoke).toHaveBeenLastCalledWith("core_command_receipt", { envelope: { ...sent.envelope, issuedAt: expect.any(String) } });
  await client.invalidateTurnSubmission(task, input, "cmd_send");
  expect(invoke).toHaveBeenLastCalledWith("core_invalidate_pending_command", { envelope: { ...sent.envelope, issuedAt: expect.any(String) } });
  expect(sent).toMatchObject({ envelope: { expectedStreamVersion: task.streamVersion, idempotencyKey: "cmd_send",
    payload: { type: "submit_turn", data: { taskId: task.taskId, ...input, executorOverride: task.preferredExecutor } } } });
  expect(invoke).toHaveBeenCalledTimes(3);
});
it("retires only the frozen context step and does not switch or submit as a side effect", async () => {
  const client = new TauriCoreClient();
  vi.mocked(invoke).mockResolvedValue({ status: "invalidated" });
  await client.confirmContextPackage(task, request, "original-context-hash", null, "cmd_context");
  const sent = vi.mocked(invoke).mock.calls[0][1] as { envelope: Record<string, unknown> };
  await client.invalidateContextPackage(task, request, "original-context-hash", null, "cmd_context");
  expect(invoke).toHaveBeenLastCalledWith("core_invalidate_pending_command", { envelope: { ...sent.envelope, issuedAt: expect.any(String) } });
  expect(invoke).toHaveBeenCalledTimes(2);
});

it("uses original upload identity for receipt lookup without retransmitting bytes", async () => {
  const client = new TauriCoreClient();
  const request = { uploadId: "11111111-1111-4111-8111-111111111111", name: "notes.txt", mimeType: "text/plain", sha256: "a".repeat(64), byteLength: 4 };
  const reference = { name: request.name, mimeType: request.mimeType, path: "/app/attachments/payload.txt", integrity: { sha256: request.sha256, byteLength: request.byteLength } };
  vi.mocked(invoke).mockResolvedValue(reference);
  expect(await client.uploadAttachment(request, "ZGF0YQ==")).toEqual(reference);
  expect(invoke).toHaveBeenLastCalledWith("core_save_attachment", { request, payloadBase64: "ZGF0YQ==" });
  expect(await client.attachmentUploadReceipt(request)).toEqual(reference);
  expect(invoke).toHaveBeenLastCalledWith("core_attachment_upload_receipt", { request });
});

it("routes shared submission registration, discovery and closure through the daemon protocol", async () => {
  const client = new TauriCoreClient();
  const intent: import("@dogcodes/protocol").SubmissionIntent = { schemaVersion: 1, operationId: "cmd_original", scope: "task:original", requestedAt: "2026-10-16T00:00:00Z",
    task, target: task.preferredExecutor, input: { prompt: "original", model: null, thinking: null, permissionMode: null, attachments: [] }, creation: null, context: null, submission: task };
  await client.submissionIntentPut(intent);
  expect(invoke).toHaveBeenLastCalledWith("core_submission_intents", { call: { type: "put", intent } });
  await client.submissionIntentGet(intent.operationId, intent);
  expect(invoke).toHaveBeenLastCalledWith("core_submission_intents", { call: { type: "get", operationId: intent.operationId, expected: intent } });
  await client.submissionIntentGet(intent.operationId);
  expect(invoke).toHaveBeenLastCalledWith("core_submission_intents", { call: { type: "get", operationId: intent.operationId, expected: null } });
  const originalStart = { operationId: intent.operationId, scope: intent.scope, taskId: task.taskId,
    projectId: task.projectId, target: intent.target, input: intent.input };
  await client.submissionIntentGet(intent.operationId, undefined, originalStart);
  expect(invoke).toHaveBeenLastCalledWith("core_submission_intents", { call: { type: "get_start", operationId: intent.operationId, request: originalStart } });
  await client.submissionIntentList();
  expect(invoke).toHaveBeenLastCalledWith("core_submission_intents", { call: { type: "list", after: null, limit: 50 } });
  await client.submissionIntentResolve(intent);
  expect(invoke).toHaveBeenLastCalledWith("core_submission_intents", { call: { type: "resolve", intent } });
  await client.submissionIntentClose(intent);
  expect(invoke).toHaveBeenLastCalledWith("core_submission_intents", { call: { type: "close", intent } });
});
