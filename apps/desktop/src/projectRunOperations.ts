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
  const parsed = JSON.parse(value) as Pending;
  if (!["core_project_runs_analyze", "core_project_run_edit"].includes(parsed.method)
    || parsed.request?.taskId !== taskId || typeof parsed.request.operationId !== "string") throw new Error(t("runOps.corrupt"));
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
  hasPending: (taskId: string) => { try { return Boolean(read(taskId)); } catch { return true; } },
  discardPending: (taskId: string) => { remove(taskId); },
  async retryPending(taskId: string): Promise<ProjectRunCatalog> {
    const operation = pending(taskId);
    if (!operation) return invoke("core_project_runs", { taskId });
    // Explicit retry first asks for the persisted receipt. Only when there is
    // no accepted receipt do we resend the original request, with its old ID.
    const receipt = await invoke<RunOperation | null>("core_project_run_operation", { taskId, operationId: operation.request.operationId });
    if (receipt) {
      const catalog = await invoke<ProjectRunCatalog>("core_project_runs", { taskId });
      remove(taskId, operation.request.operationId); return catalog;
    }
    return send(operation);
  },
  async mutate(method: Pending["method"], request: Pending["request"]): Promise<ProjectRunCatalog> {
    if (pending(request.taskId)) throw new Error(t("runOps.pending"));
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
