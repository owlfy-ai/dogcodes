import { invoke } from "@tauri-apps/api/core";
import { translate as t } from "./i18n";

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
    let item: PendingMutation | null = null;
    try { item = JSON.parse(raw) as PendingMutation; } catch { /* dropped below */ }
    if (!item?.request?.operationId || !item.request.taskId || !item.request.arguments || !["core_review_index", "core_review_discard", "core_review_ignore", "core_review_batch_files", "core_turn_undo"].includes(item.request.command)) {
      // An unreadable record can't be checked; it would only block file actions.
      localStorage.removeItem(name);
      continue;
    }
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
  constructor() { super(t("mutations.unverified")); }
}
async function receipt(value: PendingMutation): Promise<MutationReceipt | null> {
  const result = await invoke<MutationReceipt | null>("core_review_file_operation", { taskId: value.request.taskId, operationId: value.request.operationId });
  if (result && canonical(result.request) !== canonical(value.request)) throw new Error(t("mutations.receiptMismatch"));
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
/**
 * Settles a reply the caller is about to show. A failure or a partial result is
 * shown by the caller, so it is ended here and not reported again later.
 */
async function settleLive(value: PendingMutation, result: MutationReceipt): Promise<unknown> {
  const shown = result.outcome.status === "failed" || (result.outcome.status === "succeeded" && !!partialMutationResult(result.outcome.result));
  try { return settle(value, result); }
  finally { if (shown) await reviewMutations.acknowledge(value).catch(() => { /* reported on the next visit */ }); }
}
async function send(value: PendingMutation): Promise<unknown> {
  sending.add(value.request.operationId); notify();
  try {
    let result: unknown;
    try { result = await invoke(value.request.command, { ...value.request.arguments, operationId: value.request.operationId }); }
    catch (cause) {
      let status: MutationReceipt | null;
      try { status = await receipt(value); } catch { throw new MutationResultUnknown(); }
      if (status) return await settleLive(value, status);
      // The service records an operation before it touches any file, so without
      // a record nothing ran: report the refusal and drop the request.
      remove(value); throw cause;
    }
    return await settleLive(value, { request: value.request, outcome: { status: "succeeded", result: result ?? null } });
  } finally { sending.delete(value.request.operationId); notify(); }
}
export const reviewMutations = {
  entries,
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
  snapshot(taskId: string) { try { return JSON.stringify(entries(taskId).map(item => ({ ...item, sending: sending.has(item.request.operationId) }))); } catch (cause) { return JSON.stringify({ error: String(cause) }); } },
  async run<T>(command: MutationCommand, arguments_: Record<string, unknown>, taskId: string, rootId: string): Promise<T> {
    for (const item of entries(taskId).filter(item => (item.request.command === "core_turn_undo" ? mutationArguments(item).artifactId : mutationArguments(item).rootId) === rootId)) {
      // An earlier operation on the same files must be over first, so nothing
      // changes them twice. A finished one is ended now, as the person moved on.
      const result = await reviewMutations.check(item).catch(() => { throw new MutationResultUnknown(); });
      if (!result) continue;
      if (sending.has(item.request.operationId) || result.outcome.status === "running") throw new MutationResultUnknown();
      await reviewMutations.acknowledge(item).catch(() => { throw new MutationResultUnknown(); });
    }
    const value: PendingMutation = { request: { operationId: crypto.randomUUID(), taskId, command, arguments: JSON.parse(JSON.stringify(arguments_)) } };
    store(value); // Fail closed if the original request cannot survive a window restart.
    return await send(value) as T;
  },
  /**
   * Settles a kept operation from its record without showing anything when it
   * needs no attention: one that finished cleanly or never started is dropped.
   * Returns the record of one whose result the person should still see.
   */
  async check(value: PendingMutation) {
    if (sending.has(value.request.operationId)) return value.receipt ?? null;
    const result = await receipt(value);
    if (!result || (result.outcome.status === "succeeded" && !partialMutationResult(result.outcome.result))) { remove(value); return null; }
    remember(value, result); return result;
  },
  async acknowledge(value: PendingMutation) {
    let result: MutationReceipt | null;
    try { result = await invoke<MutationReceipt>("core_review_file_close", { taskId: value.request.taskId, operationId: value.request.operationId }); }
    catch { result = await receipt(value); }
    if (!result?.closed || canonical(result.request) !== canonical(value.request)) throw new Error(t("mutations.notClosed"));
    remove(value);
  },
};
