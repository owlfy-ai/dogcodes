import { ulid } from "ulid";
import type { CoreClient, ExecutorRef, ProjectId, SubmissionIntent, SubmissionIntentEntry, SubmissionResolution, TaskId, TurnSubmission } from "@dogcodes/protocol";
import { withClientOperationLock } from "./clientOperationLock";

// The service freezes each send, creates the conversation or switches the Agent
// when needed, and submits the message with fixed identities. This client only
// starts, continues or retires originals and lists the ones still pending, so a
// send started on another window or device is recoverable here as well.
export type PendingSubmission = SubmissionIntent;
export type SubmissionResult = SubmissionResolution | { status: "pending"; taskId: TaskId };

/** The service accepted and stored the original but has not finished it yet. */
export class PendingSubmissionError extends Error {
  constructor(readonly intent: SubmissionIntent, cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause));
    this.name = "PendingSubmissionError";
  }
}

/** A start whose outcome is unknown: the reply and the follow-up lookup both failed. */
export class UnconfirmedSubmissionError extends Error {
  constructor(readonly request: UnconfirmedSubmission, cause: unknown) {
    super(cause instanceof Error ? cause.message : String(cause));
    this.name = "UnconfirmedSubmissionError";
  }
}
export interface StartRequest { scope: string; taskId: TaskId | null; projectId: ProjectId; target: ExecutorRef; input: TurnSubmission }
/**
 * This client's own retry identity for a start the service may or may not have
 * received. It is kept until the service's record (or its absence) is verified,
 * so a retry reuses the original id instead of risking a second send.
 */
export interface UnconfirmedSubmission extends StartRequest { schemaVersion: 1; operationId: string; requestedAt: string }
export type UnconfirmedCheck = SubmissionResolution | { status: "pending"; taskId: TaskId | null } | { status: "not_received" };

const unconfirmedPrefix = "dogcodes:unconfirmed-send:";
function remember(record: UnconfirmedSubmission) {
  // A storage failure stops here, before anything is sent.
  localStorage.setItem(unconfirmedPrefix + record.operationId, JSON.stringify(record)); emit();
}
function forgetUnconfirmed(operationId: string) {
  if (localStorage.getItem(unconfirmedPrefix + operationId) !== null) { localStorage.removeItem(unconfirmedPrefix + operationId); emit(); }
}
function unconfirmed(): { records: UnconfirmedSubmission[]; error: string | null } {
  const records: UnconfirmedSubmission[] = []; let error: string | null = null;
  for (const key of Object.keys(localStorage).filter(key => key.startsWith(unconfirmedPrefix)).sort()) {
    try {
      const value = JSON.parse(localStorage.getItem(key) ?? "null") as UnconfirmedSubmission;
      if (value?.schemaVersion !== 1 || unconfirmedPrefix + value.operationId !== key || !text(value.scope) || !value.input) throw new Error("invalid");
      records.push(value);
    } catch { error = "部分发送结果未知的本机记录损坏，已保留原记录，请检查本机存储。"; }
  }
  return { records, error };
}

const listeners = new Set<() => void>();
const emit = () => listeners.forEach(listener => listener());
const text = (value: unknown): value is string => typeof value === "string" && value.length > 0;

interface Cache { records: PendingSubmission[]; error: string | null; flight?: Promise<void>; snapshot: string }
const caches = new WeakMap<CoreClient, Cache>();
function cache(client: CoreClient): Cache {
  let state = caches.get(client);
  if (!state) { state = { records: [], error: null, snapshot: "[]" }; caches.set(client, state); }
  return state;
}
function publish(client: CoreClient, records: PendingSubmission[], error: string | null) {
  const state = cache(client);
  state.records = records; state.error = error;
  state.snapshot = JSON.stringify(error ? { error, records } : records);
  emit();
}

function api(client: CoreClient) {
  if (!client.submissionStart || !client.submissionContinue || !client.submissionRetire || !client.submissionIntentGet || !client.submissionIntentList) {
    throw new Error("当前客户端不支持可靠发送，请更新后重试。");
  }
  return client as Required<Pick<CoreClient, "submissionStart" | "submissionContinue" | "submissionRetire" | "submissionIntentGet" | "submissionIntentList">> & CoreClient;
}
function settled(entry: SubmissionIntentEntry): SubmissionResolution {
  if (entry.state !== "closed") throw new PendingSubmissionError(entry.intent, new Error("发送结果尚未确认，请核实原发送。"));
  return entry.resolution;
}

/** Read only. Lists pending originals recorded by the service. */
function discover(client: CoreClient): Promise<void> {
  const state = cache(client);
  if (state.flight) return state.flight;
  state.flight = (async () => {
    try {
      const service = api(client);
      const records: PendingSubmission[] = [];
      let partial: string | null = null, after: string | null = null;
      const seen = new Set<string>();
      do {
        const page = await service.submissionIntentList(after, 50);
        if (!Array.isArray(page.operationIds) || page.operationIds.some(id => !text(id) || seen.has(id))
          || (page.nextCursor !== null && (!text(page.nextCursor) || page.nextCursor === after))) {
          throw new Error("后台发送记录分页无效。");
        }
        if (page.unrecognizedEntries) partial ??= "后台发送记录目录含无法识别的条目，其他记录仍可核实。";
        for (const id of page.operationIds) {
          seen.add(id);
          try {
            const entry = await service.submissionIntentGet(id);
            if (entry?.state === "pending" && entry.intent.operationId === id) records.push(entry.intent);
          } catch (cause) { partial ??= `部分后台发送记录无法核实：${String(cause)}`; }
        }
        after = page.nextCursor;
      } while (after !== null);
      publish(client, records, partial);
    } catch (cause) {
      publish(client, cache(client).records, `暂时无法同步后台发送记录：${String(cause)}。请重新连接后重试。`);
    } finally { state.flight = undefined; }
  })();
  return state.flight;
}

async function start(client: CoreClient, request: StartRequest & { operationId?: string }): Promise<SubmissionResolution> {
  const service = api(client);
  if (!text(request.scope) || (!request.input.prompt.trim() && !request.input.attachments.length)) throw new Error("发送内容无效。");
  const record: UnconfirmedSubmission = structuredClone({ schemaVersion: 1, operationId: request.operationId ?? `cmd_${ulid()}`,
    requestedAt: new Date().toISOString(), scope: request.scope, taskId: request.taskId, projectId: request.projectId, target: request.target, input: request.input });
  return withClientOperationLock(`submission:${record.scope}`, () => send(service, client, record));
}
async function send(service: ReturnType<typeof api>, client: CoreClient, record: UnconfirmedSubmission): Promise<SubmissionResolution> {
  const { schemaVersion: _schema, requestedAt: _at, ...call } = record;
  remember(record);
  try {
    const entry = await service.submissionStart(call);
    forgetUnconfirmed(record.operationId);
    return settled(entry);
  } catch (cause) {
    if (cause instanceof PendingSubmissionError) { forgetUnconfirmed(record.operationId); throw cause; }
    // The service keeps working after a lost reply; only a successful lookup of
    // this exact identity decides whether the original exists and how it ended.
    let latest: SubmissionIntentEntry | null;
    try { latest = await service.submissionIntentGet(record.operationId); }
    catch { throw new UnconfirmedSubmissionError(record, cause); }
    forgetUnconfirmed(record.operationId);
    if (latest?.state === "closed") return latest.resolution;
    if (latest?.state === "pending") throw new PendingSubmissionError(latest.intent, cause);
    throw cause; // Verified absent: the caller may return the content to the composer.
  } finally { void discover(client); }
}
/** Read only: looks up an unconfirmed start by its original identity. */
async function verifyUnconfirmed(client: CoreClient, record: UnconfirmedSubmission): Promise<UnconfirmedCheck> {
  const latest = await api(client).submissionIntentGet(record.operationId);
  if (!latest) return { status: "not_received" };
  forgetUnconfirmed(record.operationId);
  void discover(client);
  return latest.state === "closed" ? latest.resolution : { status: "pending", taskId: latest.intent.task.taskId };
}
/** Sends the original request again with its original identity; the service deduplicates. */
async function resendUnconfirmed(client: CoreClient, record: UnconfirmedSubmission): Promise<SubmissionResolution> {
  const service = api(client);
  return withClientOperationLock(`submission:${record.scope}`, () => send(service, client, record));
}
/** Only after verification found no record: stop tracking without sending. */
function abandonUnconfirmed(record: UnconfirmedSubmission) { forgetUnconfirmed(record.operationId); }

function original(action: "continue" | "retire") {
  return async (client: CoreClient, record: PendingSubmission): Promise<SubmissionResult> => {
    const service = api(client);
    return withClientOperationLock(`submission:${record.scope}`, async () => {
      try {
        const entry = action === "continue" ? await service.submissionContinue(record.operationId) : await service.submissionRetire(record.operationId);
        return entry.state === "closed" ? entry.resolution : { status: "pending", taskId: record.task.taskId };
      } finally { void discover(client); }
    });
  };
}
/** Read only: proves how the original ended, without executing anything. */
async function check(client: CoreClient, record: PendingSubmission): Promise<SubmissionResult> {
  const entry = await api(client).submissionIntentGet(record.operationId);
  if (entry?.state === "closed") { void discover(client); return entry.resolution; }
  const resolution = await client.submissionIntentResolve?.(record) ?? null;
  if (!resolution) return { status: "pending", taskId: record.task.taskId };
  // Persist the proven ending so no client lists the original again; the service re-verifies it.
  const closed = await client.submissionIntentClose?.(record);
  void discover(client);
  return closed?.state === "closed" ? closed.resolution : resolution;
}

const watchers = new WeakMap<CoreClient, { count: number; stop: () => void }>();
function watch(client: CoreClient) {
  if (!client.submissionIntentList) return () => {};
  let watcher = watchers.get(client);
  if (!watcher) {
    const refresh = () => void discover(client);
    // Notifications reduce latency; polling/focus cover lost hints.
    const timer = window.setInterval(refresh, 15_000);
    window.addEventListener("focus", refresh); window.addEventListener("online", refresh);
    const stopStream = client.subscribeSubmissionIntents?.(refresh);
    watcher = { count: 0, stop: () => {
      stopStream?.(); clearInterval(timer);
      window.removeEventListener("focus", refresh); window.removeEventListener("online", refresh);
    } };
    watchers.set(client, watcher); refresh();
  }
  watcher.count++;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    if (--watcher!.count === 0) { watcher!.stop(); watchers.delete(client); }
  };
}

export const submissionOperations = {
  start, check, resume: original("continue"), retire: original("retire"), discover, watch,
  list: (client: CoreClient) => cache(client).records,
  unconfirmed: () => JSON.stringify(unconfirmed()),
  verifyUnconfirmed, resendUnconfirmed, abandonUnconfirmed,
  snapshot: (client: CoreClient) => cache(client).snapshot,
  subscribe(listener: () => void) {
    listeners.add(listener); window.addEventListener("storage", listener);
    return () => { listeners.delete(listener); window.removeEventListener("storage", listener); };
  },
};
