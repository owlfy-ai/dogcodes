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
  return JSON.parse(saved) as ReviewStartRequest | { error: string } | null;
}
export function ReviewStartRecovery({ client, taskId, scope, busy = false, onRecovered }: {
  client: CoreClient; taskId: TaskId; scope: string; busy?: boolean; onRecovered: (run: TurnReviewRun) => void;
}) {
  const record = usePendingReview(taskId, scope);
  const pending = record && "runId" in record ? record : null;
  const [working, setWorking] = useState(false), [missing, setMissing] = useState(false), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true), context = useRef(""); context.current = `${taskId}:${scope}`;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const id = pending?.runId;
  useEffect(() => { setMissing(false); setError(""); }, [id, taskId, scope]);
  async function recover(resend: boolean) {
    if (!pending || guard.current || busy) return;
    guard.current = true; setWorking(true); setError("");
    const origin = `${taskId}:${scope}`;
    try {
      const run = resend ? await operations.resend(client, scope, pending) : await operations.check(client, scope, pending);
      if (mounted.current && context.current === origin) { setMissing(!run); if (run) onRecovered(run); }
    } catch (cause) { if (mounted.current && context.current === origin) setError(userFacingError(cause, t("reviewRecovery.verifyFailed"))); }
    finally { guard.current = false; if (mounted.current) setWorking(false); }
  }
  if (!record) return null;
  if ("error" in record) return <InlineError>{record.error}</InlineError>;
  return <div className="turn-action-feedback" role="group" aria-label={t("reviewRecovery.label")}>
    <p>{t("reviewRecovery.pending", { kind: reviewLabels[record.kind], agent: providerLabel(record.executor.providerId) })}</p>
    <button type="button" disabled={busy || working} onClick={() => void recover(false)}>{t("reviewRecovery.verify")}</button>
    {missing && <><p>{t("reviewRecovery.missing")}</p><button type="button" disabled={busy || working} onClick={() => void recover(true)}>{t("reviewRecovery.resend")}</button></>}
    <button type="button" disabled={busy || working} onClick={() => { try { operations.forget(scope, record); } catch (cause) { setError(String(cause)); } }}>{t("reviewRecovery.forget")}</button>
    <small>{t("reviewRecovery.note")}</small>
    {working && <LoadingStatus>{t("reviewRecovery.verifying")}</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
