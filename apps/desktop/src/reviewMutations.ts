import { invoke } from "@tauri-apps/api/core";

type MutationCommand = "core_review_index" | "core_review_discard" | "core_review_ignore" | "core_review_batch_files" | "core_turn_undo";
export interface MutationRequest { operationId: string; taskId: string; command: MutationCommand; arguments: Record<string, unknown> }
export interface MutationReceipt { closed?: boolean; request: MutationRequest; outcome: { status: "running" | "unknown" } | { status: "succeeded"; result: unknown } | { status: "failed"; message: string } }
export interface PendingMutation { request: MutationRequest; receipt?: MutationReceipt | null; sending?: boolean }
const prefix = "dogcodes:pending-review-mutation:";
const listeners = new Set<() => void>(), sending = new Set<string>();
const key = (id: string) => prefix + id;
function notify() { listeners.forEach(listener => listener()); }
function store(value: PendingMutation) { localStorage.setItem(key(value.request.operationId), JSON.stringify(value)); notify(); }
function remove(value: PendingMutation) { localStorage.removeItem(key(value.request.operationId)); notify(); }
function entries(taskId: string): PendingMutation[] {
  const result: PendingMutation[] = [];
  for (let i = 0; i < localStorage.length; i++) {
    const name = localStorage.key(i);
    if (!name?.startsWith(prefix)) continue;
    const raw = localStorage.getItem(name); if (!raw) continue;
    const item = JSON.parse(raw) as PendingMutation;
    if (!item.request?.operationId || !item.request.taskId || !item.request.arguments || !["core_review_index", "core_review_discard", "core_review_ignore", "core_review_batch_files", "core_turn_undo"].includes(item.request.command)) throw new Error("待核实的文件操作记录损坏，请保留记录并检查本机存储");
    if (item.request.taskId === taskId) result.push(item);
  }
  return result.sort((a, b) => a.request.operationId.localeCompare(b.request.operationId));
}
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([k, v]) => `${JSON.stringify(k)}:${canonical(v)}`).join(",")}}`;
  return JSON.stringify(value);
}
export function mutationArguments(value: PendingMutation): Record<string, unknown> {
  return (value.request.arguments.request ?? value.request.arguments) as Record<string, unknown>;
}
export function partialMutationResult(result: unknown): { completed: string[]; error: string } | null {
  if (!result || typeof result !== "object" || !("error" in result) || typeof result.error !== "string") return null;
  return { completed: "completed" in result && Array.isArray(result.completed) ? result.completed.filter((path): path is string => typeof path === "string") : [], error: result.error };
}
export class MutationResultUnknown extends Error {
  constructor() { super("文件操作结果尚未核实，请先核实原操作，避免重复修改文件"); }
}
async function receipt(value: PendingMutation): Promise<MutationReceipt | null> {
  const result = await invoke<MutationReceipt | null>("core_review_file_operation", { taskId: value.request.taskId, operationId: value.request.operationId });
  if (result && canonical(result.request) !== canonical(value.request)) throw new Error("文件操作回执与原请求不匹配，未重发操作");
  return result;
}
function remember(value: PendingMutation, result: MutationReceipt | null) {
  if (result?.closed) { remove(value); return; }
  if (localStorage.getItem(key(value.request.operationId))) store({ ...value, receipt: result });
}
function settle(value: PendingMutation, result: MutationReceipt): unknown {
  if (result.outcome.status === "succeeded") {
    if (partialMutationResult(result.outcome.result)) remember(value, result); else remove(value);
    return result.outcome.result;
  }
  remember(value, result);
  if (result.outcome.status === "failed") throw new Error(result.outcome.message);
  throw new MutationResultUnknown();
}
async function send(value: PendingMutation): Promise<unknown> {
  sending.add(value.request.operationId); notify();
  try {
    let result: unknown;
    try { result = await invoke(value.request.command, { ...value.request.arguments, operationId: value.request.operationId }); }
    catch {
      let status: MutationReceipt | null;
      try { status = await receipt(value); } catch { throw new MutationResultUnknown(); }
      if (status) return settle(value, status);
      remember(value, null); throw new MutationResultUnknown();
    }
    return settle(value, { request: value.request, outcome: { status: "succeeded", result: result ?? null } });
  } finally { sending.delete(value.request.operationId); notify(); }
}
export const reviewMutations = {
  entries,
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
  snapshot(taskId: string) { try { return JSON.stringify(entries(taskId).map(item => ({ ...item, sending: sending.has(item.request.operationId) }))); } catch (cause) { return JSON.stringify({ error: String(cause) }); } },
  async run<T>(command: MutationCommand, arguments_: Record<string, unknown>, taskId: string, rootId: string): Promise<T> {
    if (entries(taskId).some(item => (item.request.command === "core_turn_undo" ? mutationArguments(item).artifactId : mutationArguments(item).rootId) === rootId)) throw new MutationResultUnknown();
    const value: PendingMutation = { request: { operationId: crypto.randomUUID(), taskId, command, arguments: JSON.parse(JSON.stringify(arguments_)) } };
    store(value); // Fail closed if the original request cannot survive a window restart.
    return await send(value) as T;
  },
  async check(value: PendingMutation) { const result = await receipt(value); remember(value, result); return result; },
  async resend(value: PendingMutation) {
    const result = await receipt(value);
    // Even when no record is visible yet, only resend the exact original identity.
    return result ? settle(value, result) : send(value);
  },
  async acknowledge(value: PendingMutation) {
    let result: MutationReceipt | null;
    try { result = await invoke<MutationReceipt>("core_review_file_close", { taskId: value.request.taskId, operationId: value.request.operationId }); }
    catch { result = await receipt(value); }
    if (!result?.closed || canonical(result.request) !== canonical(value.request)) throw new Error("尚未确认后台已结束此次文件操作，请核实后重试。");
    remove(value);
  },
};
