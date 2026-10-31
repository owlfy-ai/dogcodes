import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { AttachmentUploadRequest, CoreClient, PromptAttachment } from "@dogcodes/protocol";
import { attachmentUploads as uploads, type PendingAttachmentUpload } from "./attachmentUploads";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

function UploadItem({ client, record, onRecover }: { client: CoreClient; record: PendingAttachmentUpload;
  onRecover: (request: AttachmentUploadRequest, reference: PromptAttachment) => void }) {
  const [working, setWorking] = useState(false), [error, setError] = useState(""), [checked, setChecked] = useState(false);
  const [reference, setReference] = useState<PromptAttachment | null>(null);
  const busy = useRef(false), mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function check() {
    if (busy.current) return;
    busy.current = true; setWorking(true); setError("");
    try {
      const result = await uploads.check(record, client);
      if (mounted.current) { setReference(result); setChecked(true); }
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, t("uploadRecovery.verifyFailed"))); }
    finally { busy.current = false; if (mounted.current) setWorking(false); }
  }
  return <div className="turn-action-feedback" role="group" aria-label={t("uploadRecovery.label")}>
    <p>{t("uploadRecovery.pending", { name: record.request.name })}</p>
    <button type="button" disabled={working} onClick={() => void check()}>{t("uploadRecovery.verify")}</button>
    {reference && <button type="button" disabled={working} onClick={() => onRecover(record.request, reference)}>{t("uploadRecovery.addToDraft")}</button>}
    {checked && !reference && <p>{t("uploadRecovery.notFound")}</p>}
    <button type="button" disabled={working} onClick={() => { try { uploads.forget(record.request.uploadId); } catch (cause) { setError(String(cause)); } }}>{t("uploadRecovery.hide")}</button>
    <small>{t("uploadRecovery.note")}</small>
    {working && <LoadingStatus>{t("uploadRecovery.verifying")}</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
export function AttachmentUploadRecovery({ client, busy, attachedIds, onRecover }: { client: CoreClient; busy: boolean; attachedIds: string[];
  onRecover: (request: AttachmentUploadRequest, reference: PromptAttachment) => void }) {
  const records = JSON.parse(useSyncExternalStore(uploads.subscribe, uploads.snapshot, () => "[]")) as PendingAttachmentUpload[] | { error: string };
  if (busy) return null;
  if (!Array.isArray(records)) return <InlineError>{records.error}</InlineError>;
  return <>{records.filter(record => !attachedIds.includes(record.request.uploadId)).map(record => <UploadItem key={record.request.uploadId} client={client} record={record} onRecover={onRecover}/>)}</>;
}
