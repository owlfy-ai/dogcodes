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
  const value = JSON.parse(raw) as ForkRequest;
  if (!value || value.taskId !== task || value.turnId !== turn || typeof value.newTaskId !== "string" || !value.newTaskId || !["workspace", "worktree"].includes(value.environment)) throw new Error(t("forkOps.corrupt"));
  return value;
}
function forget(request: ForkRequest) {
  if (read(request.taskId, request.turnId)?.newTaskId === request.newTaskId) {
    localStorage.removeItem(key(request.taskId, request.turnId)); emit();
  }
}
class ForkSourceDeleted extends Error {}
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
    throw new Error(t("forkOps.kept", { reason: cause instanceof Error ? cause.message : String(cause) }));
  }
}
export const forkOperations = {
  read, forget, check, send, available,
  async resend(client: CoreClient, request: ForkRequest) {
    const receipt = await check(client, request);
    if (receipt?.status === "running") throw new Error(t("forkOps.stillRunning"));
    return receipt?.completed ? available(receipt) : send(client, request);
  },
  snapshot(task: string, turn: string) { try { return JSON.stringify(read(task, turn)); } catch (error) { return JSON.stringify({ error: String(error) }); } },
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
};
