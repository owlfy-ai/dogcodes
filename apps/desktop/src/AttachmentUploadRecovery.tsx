import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { AttachmentUploadRequest, CoreClient, PromptAttachment } from "@dogcodes/protocol";
import { attachmentUploads as uploads, type PendingAttachmentUpload } from "./attachmentUploads";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { userFacingError } from "./userFacingError";

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
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, "暂时无法核实附件导入，请稍后重试。")); }
    finally { busy.current = false; if (mounted.current) setWorking(false); }
  }
  return <div className="turn-action-feedback" role="group" aria-label="待核实的附件导入">
    <p>{record.request.name} · 附件导入记录尚未交给发送流程。</p>
    <button type="button" disabled={working} onClick={() => void check()}>核实附件导入</button>
    {reference && <button type="button" disabled={working} onClick={() => onRecover(record.request, reference)}>添加到当前草稿</button>}
    {checked && !reference && <p>尚未查到完整附件；请重新选择原文件。未自动重新上传或发送消息。</p>}
    <button type="button" disabled={working} onClick={() => { try { uploads.forget(record.request.uploadId); } catch (cause) { setError(String(cause)); } }}>隐藏导入记录</button>
    <small>隐藏只移除本机跟踪，不删除附件文件或历史消息。</small>
    {working && <LoadingStatus>正在核实附件…</LoadingStatus>}
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
