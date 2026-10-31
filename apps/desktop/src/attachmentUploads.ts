import type { AttachmentUploadRequest, CoreClient, PromptAttachment } from "@dogcodes/protocol";
import { TauriCoreClient } from "./tauriClient";
import { withClientOperationLock } from "./clientOperationLock";
import { translate as t } from "./i18n";

const prefix = "dogcodes:pending-attachment-upload:";
const listeners = new Set<() => void>();
const native = new TauriCoreClient();
export interface PendingAttachmentUpload { schemaVersion: 1; request: AttachmentUploadRequest }
const valid = (request: AttachmentUploadRequest | undefined) => !!request
  && typeof request.uploadId === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(request.uploadId)
  && typeof request.name === "string" && request.name.length > 0 && request.name.length <= 512
  && typeof request.mimeType === "string" && request.mimeType.length > 0 && request.mimeType.length <= 128
  && typeof request.sha256 === "string" && /^[0-9a-f]{64}$/.test(request.sha256)
  && Number.isSafeInteger(request.byteLength) && request.byteLength >= 0 && request.byteLength <= 25 * 1024 * 1024;
function read(id: string): PendingAttachmentUpload | null {
  const raw = localStorage.getItem(prefix + id);
  if (raw === null) return null;
  const record = JSON.parse(raw) as PendingAttachmentUpload;
  if (!record || record.schemaVersion !== 1 || !valid(record.request) || record.request.uploadId !== id) throw new Error(t("uploads.corrupt"));
  return record;
}
function list() {
  return Object.keys(localStorage).filter(key => key.startsWith(prefix)).sort().map(key => read(key.slice(prefix.length))!);
}
const emit = () => listeners.forEach(listener => listener());
function remember(request: AttachmentUploadRequest) {
  if (!valid(request)) throw new Error(t("uploads.invalid"));
  const old = read(request.uploadId);
  if (old && JSON.stringify(old.request) !== JSON.stringify(request)) throw new Error(t("uploads.identityTaken"));
  if (!old) { localStorage.setItem(prefix + request.uploadId, JSON.stringify({ schemaVersion: 1, request })); emit(); }
}
function forget(id: string) {
  if (read(id)) { localStorage.removeItem(prefix + id); emit(); }
}
function verify(request: AttachmentUploadRequest, result: PromptAttachment): PromptAttachment {
  if (!result || result.name !== request.name || result.mimeType !== request.mimeType || typeof result.path !== "string" || !result.path
    || result.integrity?.sha256 !== request.sha256 || result.integrity.byteLength !== request.byteLength) throw new Error(t("uploads.receiptMismatch"));
  return structuredClone(result);
}
function clientOrNative(client?: CoreClient) {
  const value = client ?? native;
  if (!value.uploadAttachment || !value.attachmentUploadReceipt) throw new Error(t("uploads.unsupported"));
  return value;
}
async function check(record: PendingAttachmentUpload, client?: CoreClient) {
  if (!valid(record.request)) throw new Error(t("uploads.invalid"));
  const value = clientOrNative(client);
  const result = await value.attachmentUploadReceipt!(record.request);
  return result ? verify(record.request, result) : null;
}
async function prepare(file: File, uploadId: string, mimeType: string, client?: CoreClient): Promise<{ request: AttachmentUploadRequest; reference: PromptAttachment }> {
  if (file.size > 25 * 1024 * 1024) throw new Error(t("uploads.tooLarge"));
  const bytes = new Uint8Array(await file.arrayBuffer());
  if (bytes.length > 25 * 1024 * 1024) throw new Error(t("uploads.tooLarge"));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  const request: AttachmentUploadRequest = { uploadId, name: file.name, mimeType, byteLength: bytes.length,
    sha256: [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, "0")).join("") };
  return withClientOperationLock(prefix + uploadId, async () => {
    const value = clientOrNative(client);
    remember(request); // Before the first RPC; a reload needs only this to query.
    const found = await check({ schemaVersion: 1, request }, value);
    if (found) return { request, reference: found };
    let binary = "";
    for (let offset = 0; offset < bytes.length; offset += 32768) binary += String.fromCharCode(...bytes.subarray(offset, offset + 32768));
    let reference: PromptAttachment;
    try { reference = verify(request, await value.uploadAttachment!(request, btoa(binary))); }
    catch (cause) {
      // A missing response is not evidence that the file was not published.
      const recovered = await check({ schemaVersion: 1, request }, value);
      if (!recovered) throw cause;
      reference = recovered;
    }
    return { request, reference };
  });
}
export const attachmentUploads = {
  prepare, check, remember, forget, list,
  snapshot() { try { return JSON.stringify(list()); } catch (cause) { return JSON.stringify({ error: String(cause) }); } },
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
};
