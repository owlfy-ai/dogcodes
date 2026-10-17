import { ulid } from "ulid";
import type { ApprovalReceipt, ApprovalReceiptQuery, ApprovalSource, CoreClient, PendingApproval, QuestionAnswer, TaskId, TaskSummary } from "@dogcodes/protocol";
import { withClientOperationLock } from "./clientOperationLock";

type ApprovalTask = Pick<TaskSummary, "taskId" | "streamVersion">;
export interface PendingApprovalResponse {
  schemaVersion: 1; query: ApprovalReceiptQuery; streamVersion: number; requestedAt: string; accepted: boolean;
}
export type ApprovalCheck = { kind: "accepted"; receipt: ApprovalReceipt } | { kind: "missing" } | { kind: "obsolete" };
const prefix = "dogcodes:pending-approval:";
const key = (taskId: TaskId, source: ApprovalSource, approvalId: string) => `${prefix}${taskId}:${source.turnId}:${source.requestedAtSequence}:${encodeURIComponent(approvalId)}`;
const scope = (record: PendingApprovalResponse) => key(record.query.taskId, record.query, record.query.approvalId);
const entryKey = (record: PendingApprovalResponse) => `${scope(record)}:${record.query.operationId}`;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach(listener => listener());
function read(taskId: TaskId): PendingApprovalResponse[] {
  const keys: string[] = [];
  for (let i = 0; i < localStorage.length; i++) { const item = localStorage.key(i); if (item?.startsWith(`${prefix}${taskId}:`)) keys.push(item); }
  return keys.sort().flatMap(item => {
    const raw = localStorage.getItem(item); if (!raw) return [];
    const record = JSON.parse(raw) as PendingApprovalResponse, query = record?.query;
    if (record?.schemaVersion !== 1 || query?.taskId !== taskId || typeof query?.turnId !== "string" || !query.turnId.startsWith("trn_")
      || typeof query.approvalId !== "string" || !query.approvalId || !Number.isSafeInteger(query.requestedAtSequence) || query.requestedAtSequence < 1
      || typeof query.operationId !== "string" || !query.operationId.startsWith("cmd_") || typeof query.requestFingerprint !== "string" || !query.requestFingerprint
      || !Number.isSafeInteger(record.streamVersion) || record.streamVersion < 0 || typeof record.requestedAt !== "string" || typeof record.accepted !== "boolean"
      || item !== entryKey(record)) throw new Error("待核实的审批记录损坏，请保留记录并检查本机存储。");
    return [record];
  });
}
function current(record: PendingApprovalResponse) { return read(record.query.taskId).find(item => scope(item) === scope(record) && item.query.operationId === record.query.operationId); }
function tracked(record: PendingApprovalResponse) {
  const value = current(record);
  if (!value || value.query.operationId !== record.query.operationId || value.query.requestFingerprint !== record.query.requestFingerprint || value.streamVersion !== record.streamVersion)
    throw new Error("原审批跟踪已变化，未重发请求。");
  return value;
}
function forget(record: PendingApprovalResponse) {
  if (current(record)?.query.operationId === record.query.operationId) { localStorage.removeItem(entryKey(record)); emit(); }
}
function markAccepted(record: PendingApprovalResponse) {
  const value = current(record);
  if (value?.query.operationId === record.query.operationId && !value.accepted) {
    localStorage.setItem(entryKey(record), JSON.stringify({ ...value, accepted: true })); emit();
  }
}
function validateReceipt(record: PendingApprovalResponse, receipt: ApprovalReceipt) {
  if (!receipt?.acknowledgement?.accepted || receipt.acknowledgement.commandId !== record.query.operationId
    || receipt.acknowledgement.requestFingerprint !== record.query.requestFingerprint || !["unknown", "sent", "failed"].includes(receipt.delivery))
    throw new Error("审批回执与原请求不符，请检查原操作，未重复提交。");
  if (receipt.delivery === "sent") forget(record); else markAccepted(record);
  return { kind: "accepted", receipt } as const;
}
async function check(client: CoreClient, record: PendingApprovalResponse): Promise<ApprovalCheck> {
  if (!client.approvalReceipt) throw new Error("当前客户端不支持审批回执查询，请更新后核实。");
  const receipt = await client.approvalReceipt(record.query);
  if (receipt !== null) return validateReceipt(record, receipt);
  // A missing receipt alone does not mean an in-flight command failed. Once
  // its original version is superseded it cannot newly pass the store CAS.
  // Query again AFTER observing that version to catch an acceptance that won.
  const task = (await client.bootstrap()).tasks.find(item => item.taskId === record.query.taskId);
  if (task && task.streamVersion > record.streamVersion) {
    const latest = await client.approvalReceipt(record.query);
    if (latest !== null) return validateReceipt(record, latest);
    if (record.accepted || current(record)?.accepted) throw new Error("原审批已确认受理，但暂时无法查到回执；不会重复提交。");
    forget(record); return { kind: "obsolete" };
  }
  return { kind: "missing" };
}
async function send(client: CoreClient, record: PendingApprovalResponse, optionId: string, answer?: QuestionAnswer) {
  tracked(record);
  const { taskId, approvalId, turnId, requestedAtSequence, operationId } = record.query;
  try {
    await client.respondApproval({ taskId, streamVersion: record.streamVersion }, approvalId, optionId, { turnId, requestedAtSequence }, answer, operationId);
  } catch (cause) {
    const result = await check(client, record);
    if (result.kind === "accepted") return;
    if (result.kind === "obsolete") throw new Error("原审批未被受理，会话版本已更新。请重新查看请求，再确认提交。");
    throw cause;
  }
  markAccepted(record);
}
async function submit(client: CoreClient, task: ApprovalTask, approval: PendingApproval, optionId: string, answer?: QuestionAnswer) {
  if (!client.prepareApprovalResponse || !client.approvalReceipt) throw new Error("当前客户端不支持可靠审批，请更新后重试。");
  // Freeze transient input only. Neither the answer nor the form/option is saved.
  const original = structuredClone({ task: { taskId: task.taskId, streamVersion: task.streamVersion }, source: { turnId: approval.turnId, requestedAtSequence: approval.requestedAtSequence }, approvalId: approval.approvalId, optionId, answer });
  const storageKey = key(original.task.taskId, original.source, original.approvalId);
  return withClientOperationLock(storageKey, async () => {
    const existing = read(original.task.taskId).filter(item => scope(item) === storageKey);
    if (existing.length > 1) throw new Error("同一审批存在多个待核实请求，请先逐项核实结果。");
    let record = existing[0];
    if (record) {
      const result = await check(client, record);
      if (result.kind === "accepted") return;
      if (result.kind === "obsolete") throw new Error("原审批未被受理，会话版本已更新。请重新查看请求，再确认提交。");
      record = tracked(record);
      if (record.accepted) throw new Error("原审批已受理，投递结果待核实；不会重复提交。");
    }
    const frozenTask = { taskId: original.task.taskId, streamVersion: record?.streamVersion ?? original.task.streamVersion };
    const operationId = record?.query.operationId ?? `cmd_${ulid()}`;
    const prepared = await client.prepareApprovalResponse!(frozenTask, original.approvalId, original.optionId, original.source, original.answer, operationId);
    // Whitelist the persistable contract, even if a future host adds fields.
    const query: ApprovalReceiptQuery = { taskId: prepared.taskId, turnId: prepared.turnId, requestedAtSequence: prepared.requestedAtSequence,
      approvalId: prepared.approvalId, operationId: prepared.operationId, requestFingerprint: prepared.requestFingerprint };
    if (query.taskId !== frozenTask.taskId || query.turnId !== original.source.turnId || query.requestedAtSequence !== original.source.requestedAtSequence
      || query.approvalId !== original.approvalId || query.operationId !== operationId || typeof query.requestFingerprint !== "string" || !query.requestFingerprint) throw new Error("审批查询身份与原请求不符，未提交。");
    if (record && query.requestFingerprint !== record.query.requestFingerprint) throw new Error("选择或回答与原请求不同。请先核实原请求；重发必须使用原选择和原回答。");
    if (!record) {
      record = { schemaVersion: 1, query, streamVersion: frozenTask.streamVersion, requestedAt: new Date().toISOString(), accepted: false };
      // Keep independent recovery identities even if Web Locks are unavailable
      // in another window. Backend version checks arbitrate actual acceptance.
      if (read(original.task.taskId).some(item => scope(item) === storageKey)) throw new Error("已有同来源的审批待核实，请先核实原请求。");
      localStorage.setItem(entryKey(record), JSON.stringify(record)); emit();
    }
    await send(client, record, original.optionId, original.answer);
  });
}
export const approvalOperations = {
  submit, read, check,
  changeDecision(client: CoreClient, record: PendingApprovalResponse): Promise<ApprovalCheck> {
    return withClientOperationLock(scope(record), async () => {
      const original = tracked(record);
      if (original.accepted) throw new Error("决定已受理，不能撤回或重复提交。");
      if (!client.invalidateApprovalResponse) throw new Error("当前客户端不支持安全修改待核实的回答，请更新后重试。");
      const result = await client.invalidateApprovalResponse(original.query, original.streamVersion);
      if (result.status === "accepted") return validateReceipt(original, result.receipt);
      if (result.status !== "invalidated") throw new Error("尚未确认原请求已作废，请再次核实。");
      forget(original); return { kind: "obsolete" };
    });
  },
  /** Only an acknowledged decision may be dismissed without a version proof. */
  dismiss(record: PendingApprovalResponse) { if (!tracked(record).accepted) throw new Error("请先核实原审批结果。"); forget(record); },
  snapshot(taskId: TaskId | null) { try { return JSON.stringify(taskId ? read(taskId) : []); } catch (cause) { return JSON.stringify({ error: String(cause) }); } },
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
};
