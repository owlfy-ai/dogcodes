import type { CoreClient, ForkReceipt, ForkRequest, TaskId } from "@dogcodes/protocol";
import { translate as t } from "./i18n";

const prefix = "dogcodes:pending-fork:";
const listeners = new Set<() => void>();
const key = (task: string, turn: string) => `${prefix}${task}:${turn}`;
function emit() { listeners.forEach(listener => listener()); }
function matches(a: ForkRequest, b: ForkRequest) {
  return a.taskId === b.taskId && a.turnId === b.turnId && a.newTaskId === b.newTaskId && a.environment === b.environment;
}
function read(task: string, turn: string): ForkRequest | null {
  const raw = localStorage.getItem(key(task, turn));
  if (!raw) return null;
  let value: ForkRequest | null = null;
  try { value = JSON.parse(raw) as ForkRequest; } catch { /* dropped below */ }
  if (!value || value.taskId !== task || value.turnId !== turn || typeof value.newTaskId !== "string" || !value.newTaskId || !["workspace", "worktree"].includes(value.environment)) {
    // An unreadable record can't be checked or resent; it would only block forking.
    localStorage.removeItem(key(task, turn));
    return null;
  }
  return value;
}
function forget(request: ForkRequest) {
  if (read(request.taskId, request.turnId)?.newTaskId === request.newTaskId) {
    localStorage.removeItem(key(request.taskId, request.turnId)); emit();
  }
}
class ForkSourceDeleted extends Error {}
/** A fork request kept for recovery: its reply was lost or it did not finish. */
export class ForkPending extends Error {}
async function check(client: CoreClient, request: ForkRequest): Promise<ForkReceipt | null> {
  if (client.taskDeletion) {
    const deleted = await client.taskDeletion(request.taskId);
    if (deleted) {
      if (deleted.taskId !== request.taskId) throw new Error(t("forkOps.deletionMismatch"));
      forget(request);
      throw new ForkSourceDeleted(t("forkOps.sourceDeleted"));
    }
  }
  if (!client.forkReceipt) throw new Error(t("forkOps.receiptUnsupported"));
  const receipt = await client.forkReceipt(request.taskId, request.newTaskId);
  if (receipt && (!receipt.request || !matches(receipt.request, request))) throw new Error(t("forkOps.receiptMismatch"));
  return receipt;
}
function available(receipt: ForkReceipt): TaskId {
  if (!receipt.completed) throw new Error(t("forkOps.notCompleted"));
  if (!receipt.taskAvailable) throw new Error(t("forkOps.targetUnavailable"));
  return receipt.request.newTaskId;
}
async function send(client: CoreClient, request: ForkRequest): Promise<TaskId> {
  if (!client.forkTask) throw new Error(t("forkOps.unsupported"));
  const previous = read(request.taskId, request.turnId);
  if (previous && !matches(previous, request)) throw new Error(t("forkOps.verifyFirst"));
  localStorage.setItem(key(request.taskId, request.turnId), JSON.stringify(request)); emit();
  try {
    const id = await client.forkTask(request);
    if (id !== request.newTaskId) throw new Error(t("forkOps.otherTask"));
    return id;
  } catch (cause) {
    let receipt: ForkReceipt | null = null;
    try { receipt = await check(client, request); } catch (error) { if (error instanceof ForkSourceDeleted) throw error; /* Preserve pending intent for explicit recovery. */ }
    if (receipt?.completed) return available(receipt);
    throw new ForkPending(cause instanceof Error ? cause.message : String(cause));
  }
}
/**
 * Settles a kept fork from its receipt without asking: a finished one (opened or
 * not) or one whose source was deleted is dropped. Returns the receipt when it
 * is still running or needs the person to retry or drop it.
 */
async function settle(client: CoreClient, request: ForkRequest): Promise<ForkReceipt | null> {
  let receipt: ForkReceipt | null;
  try { receipt = await check(client, request); } catch (error) { if (error instanceof ForkSourceDeleted) return null; throw error; }
  if (receipt?.completed) { forget(request); return null; }
  return receipt;
}
export const forkOperations = {
  read, forget, check, send, available, settle,
  async resend(client: CoreClient, request: ForkRequest) {
    const receipt = await check(client, request);
    if (receipt?.status === "running") throw new Error(t("forkOps.stillRunning"));
    return receipt?.completed ? available(receipt) : send(client, request);
  },
  snapshot(task: string, turn: string) { try { return JSON.stringify(read(task, turn)); } catch (error) { return JSON.stringify({ error: String(error) }); } },
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
};
