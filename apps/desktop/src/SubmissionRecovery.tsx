import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { CoreClient } from "@dogcodes/protocol";
import { submissionOperations as operations, type PendingSubmission, type SubmissionResult, type UnconfirmedSubmission, type UnconfirmedCheck } from "./submissionOperations";
import { LoadingStatus } from "./LoadingStatus";
import { InlineError } from "./InlineError";
import { providerLabel } from "./agentPresentation";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

export function usePendingSubmissions(client: CoreClient) {
  useEffect(() => operations.watch(client), [client]);
  const snapshot = useCallback(() => operations.snapshot(client), [client]);
  return JSON.parse(useSyncExternalStore(operations.subscribe, snapshot, () => "[]")) as PendingSubmission[] | { error: string; records: PendingSubmission[] };
}
/** Sends whose outcome is unknown on this client, kept with their original identity. */
export function useUnconfirmedSubmissions() {
  return JSON.parse(useSyncExternalStore(operations.subscribe, operations.unconfirmed, () => '{"records":[],"error":null}')) as { records: UnconfirmedSubmission[]; error: string | null };
}
function UnconfirmedItem({ client, record, busy, onResolved, onRestore }: {
  client: CoreClient; record: UnconfirmedSubmission; busy: boolean;
  onResolved: (record: UnconfirmedSubmission, result: Exclude<UnconfirmedCheck, { status: "not_received" }>) => void;
  onRestore: (record: UnconfirmedSubmission) => void;
}) {
  const [working, setWorking] = useState(false), [missing, setMissing] = useState(false), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function run(action: "verify" | "resend") {
    if (busy || guard.current) return;
    guard.current = true; setWorking(true); setError("");
    try {
      const result = action === "verify" ? await operations.verifyUnconfirmed(client, record) : await operations.resendUnconfirmed(client, record);
      if (result.status === "not_received") { if (mounted.current) setMissing(true); }
      else onResolved(record, result);
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, t("submissionRecovery.verifyFailed"))); }
    finally { guard.current = false; if (mounted.current) setWorking(false); }
  }
  return <div className="turn-action-feedback" role="group" aria-label={t("submissionRecovery.unknownRequest")}>
    <p>{providerLabel(record.target.providerId)} · {new Date(record.requestedAt).toLocaleString()}</p>
    <p>{t("submissionRecovery.unknownDescription")}</p>
    <details><summary>{t("submissionRecovery.viewOriginal")}</summary><p className="submission-original-prompt">{record.input.prompt}</p>
      {!!record.input.attachments.length && <p>{t("submissionRecovery.attachments", { names: record.input.attachments.map(item => item.name).join(t("submissionRecovery.nameSeparator")) })}</p>}
    </details>
    <button type="button" disabled={busy || working} onClick={() => void run("verify")}>{t("submissionRecovery.verify")}</button>
    {missing && <><p>{t("submissionRecovery.notFound")}</p>
      <button type="button" disabled={busy || working} onClick={() => void run("resend")}>{t("submissionRecovery.resend")}</button>
      <button type="button" disabled={busy || working} onClick={() => { operations.abandonUnconfirmed(record); onRestore(record); }}>{t("submissionRecovery.abandonRestore")}</button>
    </>}
    <small>{t("submissionRecovery.resendNote")}</small>
    {working && <LoadingStatus>{t("submissionRecovery.verifying")}</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
function RecoveryItem({ client, record, busy, canResume, onResolved }: {
  client: CoreClient; record: PendingSubmission; busy: boolean; canResume: boolean;
  onResolved: (record: PendingSubmission, result: SubmissionResult) => void;
}) {
  const [working, setWorking] = useState(false), [checked, setChecked] = useState(false), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function recover(action: "check" | "resume" | "retire") {
    if (busy || guard.current || (action === "resume" && !canResume)) return;
    guard.current = true; setWorking(true); setError("");
    try {
      const result = await operations[action](client, record);
      if (result.status === "submitted" || result.status === "invalidated" || result.status === "task_deleted") onResolved(record, result);
      else if (mounted.current) setChecked(true);
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, t("submissionRecovery.verifyFailed"))); }
    finally { guard.current = false; if (mounted.current) setWorking(false); }
  }
  return <div className="turn-action-feedback" role="group" aria-label={t("submissionRecovery.pendingRequest")}>
    <p>{record.task.title} · {providerLabel(record.target.providerId)} · {new Date(record.requestedAt).toLocaleString()}</p>
    <p>{t("submissionRecovery.pendingDescription")}</p>
    <details><summary>{t("submissionRecovery.viewOriginal")}</summary><p className="submission-original-prompt">{record.input.prompt}</p>
      {!!record.input.attachments.length && <p>{t("submissionRecovery.attachments", { names: record.input.attachments.map(item => item.name).join(t("submissionRecovery.nameSeparator")) })}</p>}
      <small>{t("submissionRecovery.options", { model: record.input.model ?? t("modelPicker.default"), thinking: record.input.thinking ?? t("modelPicker.default"), permission: record.input.permissionMode ?? t("modelPicker.default") })}</small>
    </details>
    <button type="button" disabled={busy || working} onClick={() => void recover("check")}>{t("submissionRecovery.verify")}</button>
    {checked && <><p>{t("submissionRecovery.notAccepted")}</p>
      <button type="button" disabled={busy || working || !canResume} onClick={() => void recover("resume")}>{t("submissionRecovery.resume")}</button>
      {!canResume && <small>{t("submissionRecovery.restoreAgentFirst")}</small>}
    </>}
    <button type="button" disabled={busy || working} onClick={() => void recover("retire")}>{t("submissionRecovery.retire")}</button>
    <small>{t("submissionRecovery.retireNote")}</small>
    {working && <LoadingStatus>{t("submissionRecovery.verifying")}</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
export function SubmissionRecovery({ client, busy, canResume, onResolved, onUnconfirmedResolved, onRestore }: {
  client: CoreClient; busy: boolean; canResume: (record: PendingSubmission) => boolean;
  onResolved: (record: PendingSubmission, result: SubmissionResult) => void;
  onUnconfirmedResolved?: (record: UnconfirmedSubmission, result: Exclude<UnconfirmedCheck, { status: "not_received" }>) => void;
  onRestore?: (record: UnconfirmedSubmission) => void;
}) {
  const records = usePendingSubmissions(client);
  const unknown = useUnconfirmedSubmissions();
  const [notice, setNotice] = useState("");
  const mounted = useRef(true), activeClient = useRef(client);
  activeClient.current = client;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  // A normally progressing first attempt is already represented by the composer.
  // Show recovery only after it settles, not a warning during every healthy send.
  if (busy) return null;
  const readable = Array.isArray(records) ? records : records.records;
  return <>{unknown.error && <InlineError>{unknown.error}</InlineError>}
    {unknown.records.map(record => <UnconfirmedItem key={record.operationId} client={client} record={record} busy={busy}
      onResolved={(record, result) => {
        if (!mounted.current || activeClient.current !== client) return;
        setNotice(t(result.status === "submitted" ? "submissionRecovery.accepted" : result.status === "pending" ? "submissionRecovery.received"
          : result.status === "task_deleted" ? "submissionRecovery.taskDeleted" : "submissionRecovery.voided"));
        onUnconfirmedResolved?.(record, result);
      }} onRestore={record => onRestore?.(record)}/>)}
    {!Array.isArray(records) && <div><InlineError>{records.error}</InlineError>
    {client.submissionIntentList && <button type="button" onClick={() => void operations.discover(client)}>{t("submissionRecovery.rediscover")}</button>}</div>}
    {readable.map(record => <RecoveryItem key={record.operationId} client={client} record={record} busy={busy} canResume={canResume(record)} onResolved={(record, result) => {
    if (!mounted.current || activeClient.current !== client) return;
    setNotice(t(result.status === "submitted" ? "submissionRecovery.accepted"
      : result.status === "task_deleted" ? "submissionRecovery.taskDeleted" : "submissionRecovery.abandoned"));
    onResolved(record, result);
  }}/>)}{notice && <p role="status">{notice} <button type="button" onClick={() => setNotice("")}>{t("submissionRecovery.dismiss")}</button></p>}</>;
}
