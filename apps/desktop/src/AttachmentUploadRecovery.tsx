import { useEffect, useSyncExternalStore } from "react";
import { attachmentUploads as uploads, type PendingAttachmentUpload } from "./attachmentUploads";

/**
 * Drops upload records no draft uses any more: an attachment removed from the
 * draft, or one left by a window that closed before sending. Nothing is shown.
 * Uploads are keyed by their ID, so choosing the file again never reuses or
 * duplicates a half-finished one; a sent message keeps its own copy.
 */
export function AttachmentUploadRecovery({ busy, attachedIds }: { busy: boolean; attachedIds: string[] }) {
  const records = JSON.parse(useSyncExternalStore(uploads.subscribe, uploads.snapshot, () => "[]")) as PendingAttachmentUpload[] | { error: string };
  const unused = Array.isArray(records) ? records.map(record => record.request.uploadId).filter(id => !attachedIds.includes(id)) : [];
  const key = unused.join(",");
  useEffect(() => {
    if (busy || !key) return;
    for (const id of key.split(",")) { try { uploads.forget(id); } catch { /* unreadable records are dropped by the store */ } }
  }, [busy, key]);
  return null;
}
