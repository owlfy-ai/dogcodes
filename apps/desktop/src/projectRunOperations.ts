import { invoke } from "@tauri-apps/api/core";
import type { ProjectRunCatalog } from "./projectRunClient";
import { translate as t } from "./i18n";
export interface RunOperation { operationId: string; generation: string; status: "running" | "succeeded" | "failed" | "interrupted" | "superseded"; error: string | null }
type Pending = { method: "core_project_runs_analyze" | "core_project_run_edit"; request: Record<string, unknown> & { taskId: string; operationId: string } };
const prefix = (taskId: string) => `dogcodes.project-run.pending.${taskId}.`;
const memory = new Map<string, string>();
function keys(taskId: string) {
  const all = typeof localStorage === "undefined" ? [...memory.keys()] : Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index)!);
  return all.filter((key): key is string => typeof key === "string" && key.startsWith(prefix(taskId))).sort();
}
function read(taskId: string) {
  const key = keys(taskId)[0]; if (!key) return null;
  return typeof localStorage === "undefined" ? memory.get(key) ?? null : localStorage.getItem(key);
}
function remove(taskId: string, operationId?: string) {
  const key = operationId ? prefix(taskId) + operationId : keys(taskId)[0]; if (!key) return;
  if (typeof localStorage === "undefined") memory.delete(key); else localStorage.removeItem(key);
}
function store(taskId: string, operation: Pending) {
  const key = prefix(taskId) + operation.request.operationId;
  const value = JSON.stringify(operation);
  if (typeof localStorage === "undefined") memory.set(key, value); else localStorage.setItem(key, value);
}
function pending(taskId: string): Pending | null {
  const value = read(taskId); if (!value) return null;
  let parsed: Pending | null = null;
  try { parsed = JSON.parse(value) as Pending; } catch { /* dropped below */ }
  if (!parsed || !["core_project_runs_analyze", "core_project_run_edit"].includes(parsed.method)
    || parsed.request?.taskId !== taskId || typeof parsed.request.operationId !== "string") {
    // An unreadable record can't be checked; it would only block run commands.
    remove(taskId);
    return null;
  }
  return parsed;
}
export class RunOperationRejected extends Error {
  constructor(readonly code: string, message: string) { super(message); }
}
function rejection(error: unknown): RunOperationRejected | null {
  if (!error || typeof error !== "object") return null;
  const value = error as Record<string, unknown>;
  return value.kind === "rejected" && typeof value.code === "string" && typeof value.message === "string"
    ? new RunOperationRejected(value.code, value.message) : null;
}
function readableError(error: unknown): unknown {
  if (error && typeof error === "object" && "message" in error && typeof error.message === "string") return new Error(error.message);
  return error;
}
export const runOperations = {
  hasPending: (taskId: string) => Boolean(read(taskId)),
  /**
   * Settles an operation whose reply was lost (a closed window or a dropped
   * connection) from the service's record, without asking. One the service
   * received carries on there; one it never received did nothing. Either way the
   * local record is dropped, and nothing is resent.
   */
  async settlePending(taskId: string): Promise<void> {
    const operation = pending(taskId);
    if (!operation) return;
    await invoke<RunOperation | null>("core_project_run_operation", { taskId, operationId: operation.request.operationId });
    remove(taskId, operation.request.operationId);
  },
  async mutate(method: Pending["method"], request: Pending["request"]): Promise<ProjectRunCatalog> {
    if (pending(request.taskId)) {
      // Settle the earlier operation first so this one is not mixed up with it.
      try { await runOperations.settlePending(request.taskId); } catch { throw new Error(t("runOps.pending")); }
    }
    // Persist before invoking: a closed window must not lose the identity of an
    // operation already delivered to the service.
    store(request.taskId, { method, request });
    return send({ method, request });
  },
};
async function send(operation: Pending): Promise<ProjectRunCatalog> {
  const { method, request } = operation;
  try {
    const result = await invoke<ProjectRunCatalog>(method, { request });
    remove(request.taskId, request.operationId); return result;
  } catch (error) {
    const rejected = rejection(error);
    if (rejected) {
      // The service rejected this request before committing it. Do not turn a
      // known conflict into an uncertain operation or clear another window's ID.
      remove(request.taskId, request.operationId);
      throw rejected;
    }
    try {
      const receipt = await invoke<RunOperation | null>("core_project_run_operation", { taskId: request.taskId, operationId: request.operationId });
      if (receipt) {
        const result = await invoke<ProjectRunCatalog>("core_project_runs", { taskId: request.taskId });
        remove(request.taskId, request.operationId); return result;
      }
    } catch { /* retain the same durable client request */ }
    throw readableError(error);
  }
}
