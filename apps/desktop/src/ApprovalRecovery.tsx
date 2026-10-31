import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { CoreClient, TaskId } from "@dogcodes/protocol";
import { approvalOperations as operations, type ApprovalCheck, type PendingApprovalResponse } from "./approvalOperations";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

export function usePendingApprovalResponses(taskId: TaskId | null) {
  const saved = useSyncExternalStore(operations.subscribe, () => operations.snapshot(taskId), () => "[]");
  return JSON.parse(saved) as PendingApprovalResponse[] | { error: string };
}
function RecoveryItem({ client, record, busy, revision, onSettled }: { client: CoreClient; record: PendingApprovalResponse; busy: boolean; revision: number; onSettled: (result: ApprovalCheck) => void }) {
  const [working, setWorking] = useState(false), [result, setResult] = useState<ApprovalCheck | null>(null), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true), refreshAgain = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function check(changeDecision = false, automatic = false) {
    if (guard.current || busy) { if (automatic && !busy) refreshAgain.current = true; return; }
    guard.current = true; setWorking(true); setError("");
    try {
      const value = changeDecision ? await operations.changeDecision(client, record) : await operations.check(client, record);
      onSettled(value); // Matched success can remove this row before resolving.
      if (mounted.current) setResult(value);
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, t("approvalRecovery.verifyFailed"))); }
    finally {
      guard.current = false;
      if (mounted.current) {
        setWorking(false);
        if (refreshAgain.current) { refreshAgain.current = false; void check(false, true); }
      }
    }
  }
  // Reuse the task projection's revision, not a second event subscription. A
  // delivery event racing an older lookup gets one trailing read-only refresh.
  useEffect(() => { if (!busy) void check(false, true); }, [client, record.query.operationId, busy, revision]);
  const accepted = result?.kind === "accepted" || record.accepted;
  const failed = result?.kind === "accepted" && result.receipt.delivery === "failed";
  return <div className="turn-action-feedback" role="group" aria-label={t("approvalRecovery.label")}>
    <p>{t("approvalRecovery.submittedAt", { time: new Date(record.requestedAt).toLocaleString() })}</p>
    <p>{t(failed ? "approvalRecovery.deliveryFailed"
      : accepted ? "approvalRecovery.accepted"
      : result?.kind === "missing" ? "approvalRecovery.missing"
      : "approvalRecovery.verifying")}</p>
    <button type="button" disabled={busy || working} onClick={() => void check()}>{t("approvalRecovery.verify")}</button>
    {!accepted && result?.kind === "missing" && <button type="button" disabled={busy || working} onClick={() => void check(true)}>{t("approvalRecovery.change")}</button>}
    {record.accepted && <button type="button" disabled={busy || working} onClick={() => { try { operations.dismiss(record); } catch (cause) { setError(String(cause)); } }}>{t("approvalRecovery.dismiss")}</button>}
    <small>{t("approvalRecovery.note")}</small>
    {working && <LoadingStatus>{t("approvalRecovery.verifyingResult")}</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
export function ApprovalRecovery({ client, taskId, busy, revision = 0, onAccepted }: { client: CoreClient; taskId: TaskId; busy: boolean; revision?: number; onAccepted: (record: PendingApprovalResponse) => void }) {
  const pending = usePendingApprovalResponses(taskId);
  const [notice, setNotice] = useState("");
  const mounted = useRef(true), scope = useRef({ client, taskId, revision: 0 });
  if (scope.current.client !== client || scope.current.taskId !== taskId) scope.current = { client, taskId, revision: scope.current.revision + 1 };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => setNotice(""), [client, taskId]);
  const origin = scope.current;
  const settled = (record: PendingApprovalResponse, result: ApprovalCheck) => {
    if (!mounted.current || scope.current !== origin || result.kind === "missing") return;
    if (result.kind === "obsolete") setNotice(t("approvalOps.obsolete"));
    else setNotice("");
    onAccepted(record);
  };
  if (!Array.isArray(pending)) return <InlineError>{pending.error}</InlineError>;
  return <>{notice && <p role="status" className="turn-action-feedback">{notice}</p>}
    {pending.map(record => <RecoveryItem key={`${origin.revision}:${record.query.operationId}`} client={client} record={record} busy={busy} revision={revision} onSettled={result => settled(record, result)}/>)}</>;
}
