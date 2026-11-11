import { ulid } from "ulid";
import type { CoreClient, TaskId, TaskSummary } from "@dogcodes/protocol";
import { withClientOperationLock } from "./clientOperationLock";
import { translate as t } from "./i18n";

export interface PendingCancellation { schemaVersion: 1; operationId: string; task: TaskSummary; requestedAt: string }
const prefix = "dogcodes:pending-cancel:";
const key = (taskId: string, runId: string) => `${prefix}${taskId}:${runId}`;
const scope = (record: PendingCancellation) => key(record.task.taskId, record.task.activePrimaryRun!.runId);
const listeners = new Set<() => void>();
const emit = () => listeners.forEach(listener => listener());
function read(taskId: TaskId): PendingCancellation[] {
  const keys: string[] = [];
  for (let index = 0; index < localStorage.length; index++) {
    const item = localStorage.key(index);
    if (item?.startsWith(`${prefix}${taskId}:`)) keys.push(item);
  }
  return keys.sort().flatMap(item => {
    const raw = localStorage.getItem(item); if (!raw) return [];
    let record: PendingCancellation | null = null;
    try { record = JSON.parse(raw) as PendingCancellation; } catch { /* dropped below */ }
    const task = record?.task, active = task?.activePrimaryRun;
    if (record?.schemaVersion !== 1 || typeof record.operationId !== "string" || !record.operationId
      || task?.taskId !== taskId || !Number.isSafeInteger(task.streamVersion) || task.streamVersion < 0
      || !active || typeof active.runId !== "string" || !active.runId || typeof active.turnId !== "string" || !active.turnId
      || item !== key(taskId, active.runId) || typeof record.requestedAt !== "string") {
      // An unreadable record can't be checked or resent; it only blocks stopping.
      localStorage.removeItem(item);
      return [];
    }
    return [record];
  });
}
function tracked(record: PendingCancellation) {
  const current = read(record.task.taskId).find(item => scope(item) === scope(record));
  if (!current || current.operationId !== record.operationId) throw new Error(t("cancelOps.trackingChanged"));
  return current;
}
function forget(record: PendingCancellation) {
  const current = read(record.task.taskId).find(item => scope(item) === scope(record));
  if (current?.operationId === record.operationId) { localStorage.removeItem(scope(record)); emit(); }
}
async function check(client: CoreClient, record: PendingCancellation): Promise<boolean> {
  if (!client.cancelTurnReceipt) throw new Error(t("cancelOps.receiptUnsupported"));
  const receipt = await client.cancelTurnReceipt(record.task, record.operationId);
  if (receipt === null) return false;
  if (!receipt?.accepted || !receipt.requestFingerprint) throw new Error(t("cancelOps.receiptUnclear"));
  forget(record); return true;
}
async function send(client: CoreClient, record: PendingCancellation, recovering = true) {
  if (recovering && await check(client, record)) return;
  tracked(record);
  try { await client.cancelTurn(record.task, record.operationId); }
  catch (cause) { if (!await check(client, record)) throw cause; }
  forget(record);
}
async function start(client: CoreClient, task: TaskSummary) {
  const frozen = structuredClone(task);
  if (!frozen.activePrimaryRun) throw new Error(t("cancelOps.noRun"));
  if (!client.cancelTurnReceipt) throw new Error(t("cancelOps.unsupported"));
  const storageKey = key(task.taskId, frozen.activePrimaryRun.runId);
  return withClientOperationLock(storageKey, async () => {
    // Stopping the same run again reuses the unconfirmed request: the service
    // runs one ID once, so this cannot stop anything twice.
    const existing = read(task.taskId).find(item => scope(item) === storageKey);
    if (existing) { await send(client, existing); return; }
    const record: PendingCancellation = { schemaVersion: 1, operationId: `cmd_${ulid()}`, task: frozen, requestedAt: new Date().toISOString() };
    localStorage.setItem(storageKey, JSON.stringify(record)); emit();
    // This newly generated ID has never been dispatched. Avoid an unnecessary
    // query round trip while streaming events advance the original version.
    await send(client, record, false);
  });
}
/**
 * Settles a stop request whose reply was lost, without showing anything: a
 * recorded receipt means it was accepted; a run that is no longer active needs
 * no stop. Returns whether the request was accepted.
 */
async function settle(client: CoreClient, record: PendingCancellation, activeRunId: string | null | undefined): Promise<boolean> {
  if (await check(client, record)) return true;
  if (activeRunId !== undefined && activeRunId !== record.task.activePrimaryRun?.runId) forget(record);
  return false;
}
export const cancelOperations = {
  start, read, check, forget, settle,
  resend(client: CoreClient, record: PendingCancellation) { return withClientOperationLock(scope(record), () => send(client, tracked(record))); },
  snapshot(taskId: TaskId | null) { try { return JSON.stringify(taskId ? read(taskId) : []); } catch (cause) { return JSON.stringify({ error: String(cause) }); } },
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
};
