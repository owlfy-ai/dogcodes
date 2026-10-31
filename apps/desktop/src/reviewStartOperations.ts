import type { CoreClient, ProjectReviewRequest, TaskId, TurnReviewRequest, TurnReviewRun } from "@dogcodes/protocol";
import { translate as t } from "./i18n";

export type ReviewStartRequest = TurnReviewRequest | ProjectReviewRequest;
const prefix = "dogcodes:pending-review-start:";
const listeners = new Set<() => void>();
const key = (task: string, scope: string) => `${prefix}${task}:${scope}`;
function emit() { listeners.forEach(listener => listener()); }
function identity(request: TurnReviewRun["request"]) {
  return JSON.stringify([request.taskId, request.turnId, request.runId, request.kind, request.executor.providerId,
    request.executor.installationId ?? null, request.retryOf ?? null, request.sourceSnapshot ?? null,
    request.recheckOf?.runId ?? null, request.recheckOf?.repairTurnId ?? null]);
}
function read(task: string, scope: string): ReviewStartRequest | null {
  const raw = localStorage.getItem(key(task, scope));
  if (!raw) return null;
  const value = JSON.parse(raw) as ReviewStartRequest;
  if (!value || value.taskId !== task || typeof value.runId !== "string" || !value.runId || !["code", "security", "performance"].includes(value.kind)
    || typeof value.executor?.providerId !== "string" || !value.executor.providerId
    || (value.executor.installationId != null && typeof value.executor.installationId !== "string")
    || (value.retryOf != null && typeof value.retryOf !== "string")
    || (value.recheckOf != null && (typeof value.recheckOf.runId !== "string" || typeof value.recheckOf.repairTurnId !== "string"))
    || (value.turnId === null ? !value.sourceSnapshot || typeof value.sourceSnapshot !== "string" : !value.turnId || typeof value.turnId !== "string")) throw new Error(t("reviewOps.corrupt"));
  return value;
}
function forget(scope: string, request: ReviewStartRequest) {
  if (read(request.taskId, scope)?.runId === request.runId) { localStorage.removeItem(key(request.taskId, scope)); emit(); }
}
function validate(request: ReviewStartRequest, run: TurnReviewRun) {
  if (identity(run.request) !== identity(request)) throw new Error(t("reviewOps.receiptMismatch"));
}
async function check(client: CoreClient, scope: string, request: ReviewStartRequest) {
  if (!client.turnReviews) throw new Error(t("reviewOps.receiptUnsupported"));
  const runs = await client.turnReviews(request.taskId);
  const found = runs.find(run => run.request.runId === request.runId);
  if (!found) return null;
  validate(request, found); forget(scope, request); return found;
}
async function send(client: CoreClient, scope: string, request: ReviewStartRequest) {
  const previous = read(request.taskId, scope);
  if (previous && identity(previous) !== identity(request)) throw new Error(t("reviewOps.verifyFirst"));
  if (request.turnId === null ? !client.startProjectReview : !client.startTurnReview) throw new Error(t("reviewOps.unsupported"));
  localStorage.setItem(key(request.taskId, scope), JSON.stringify(request)); emit();
  try {
    const run = request.turnId === null ? await client.startProjectReview!(request) : await client.startTurnReview!(request);
    validate(request, run); forget(scope, request); return run;
  } catch (cause) {
    try { const result = await check(client, scope, request); if (result) return result; } catch { /* Keep original payload for explicit recovery. */ }
    throw new Error(t("reviewOps.kept", { reason: cause instanceof Error ? cause.message : String(cause) }));
  }
}
export const reviewStartOperations = {
  read, forget, check, send,
  async resend(client: CoreClient, scope: string, request: ReviewStartRequest) { return await check(client, scope, request) ?? send(client, scope, request); },
  snapshot(task: TaskId, scope: string) { try { return JSON.stringify(read(task, scope)); } catch (error) { return JSON.stringify({ error: String(error) }); } },
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
};
