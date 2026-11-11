import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { CoreClient, TaskId, TurnReviewRun } from "@dogcodes/protocol";
import { reviewStartOperations as operations, type ReviewStartRequest } from "./reviewStartOperations";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { providerLabel } from "./agentPresentation";
import { reviewLabels } from "./ReviewActions";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

export function usePendingReview(task: TaskId, scope: string) {
  const saved = useSyncExternalStore(operations.subscribe, () => operations.snapshot(task, scope), () => "null");
  const value = JSON.parse(saved) as ReviewStartRequest | { error: string } | null;
  return value && "runId" in value ? value : null;
}

/**
 * Finishes a review start whose reply was lost or that failed. It first looks
 * the review up and retries the same request once on its own (the service starts
 * one ID only once); only if that fails does it say so and let the person retry
 * or drop it.
 */
export function ReviewStartRecovery({ client, taskId, scope, busy = false, onRecovered }: {
  client: CoreClient; taskId: TaskId; scope: string; busy?: boolean; onRecovered: (run: TurnReviewRun) => void;
}) {
  const pending = usePendingReview(taskId, scope);
  const [working, setWorking] = useState(false), [failure, setFailure] = useState<{ runId: string; message: string } | null>(null);
  const guard = useRef(false), mounted = useRef(true), tried = useRef(new Set<string>()), context = useRef(""); context.current = `${taskId}:${scope}`;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function recover(record: ReviewStartRequest) {
    if (guard.current || busy) return;
    guard.current = true; setWorking(true);
    const origin = `${taskId}:${scope}`;
    try {
      const run = await operations.resend(client, scope, record);
      if (mounted.current && context.current === origin) { setFailure(null); onRecovered(run); }
    } catch (cause) {
      if (mounted.current && context.current === origin) setFailure({ runId: record.runId, message: userFacingError(cause, t("reviewRecovery.failed")) });
    } finally { guard.current = false; if (mounted.current) setWorking(false); }
  }
  const id = pending?.runId;
  useEffect(() => {
    if (!pending || busy || tried.current.has(pending.runId)) return;
    tried.current.add(pending.runId);
    void recover(pending);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per kept request
  }, [id, busy, taskId, scope]);
  if (!pending || failure?.runId !== pending.runId) return null;
  return <div className="turn-action-feedback" role="group" aria-label={t("reviewRecovery.label")}>
    <p>{t("reviewRecovery.notStarted", { kind: reviewLabels[pending.kind], agent: providerLabel(pending.executor.providerId) })}</p>
    <InlineError>{failure.message}</InlineError>
    <button type="button" disabled={busy || working} onClick={() => void recover(pending)}>{t("reviewRecovery.retry")}</button>
    <button type="button" disabled={busy || working} onClick={() => { operations.forget(scope, pending); setFailure(null); }}>{t("reviewRecovery.discard")}</button>
    {working && <LoadingStatus>{t("reviewRecovery.starting")}</LoadingStatus>}
  </div>;
}
