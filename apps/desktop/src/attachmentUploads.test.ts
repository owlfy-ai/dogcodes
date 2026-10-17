// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { webcrypto } from "node:crypto";
import type { AttachmentUploadRequest, CoreClient, PromptAttachment } from "@dogcodes/protocol";
import { attachmentUploads as uploads } from "./attachmentUploads";

beforeEach(() => { localStorage.clear(); vi.stubGlobal("crypto", webcrypto); });
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });
function file(text = "original") {
  const file = new File([text], "file.txt", { type: "text/plain" });
  Object.defineProperty(file, "arrayBuffer", { value: async () => new TextEncoder().encode(text).buffer });
  return file;
}
function fixture() {
  const saved = new Map<string, { request: AttachmentUploadRequest; reference: PromptAttachment }>();
  const attachmentUploadReceipt = vi.fn(async (request: AttachmentUploadRequest) => {
    const found = saved.get(request.uploadId);
    if (found && JSON.stringify(found.request) !== JSON.stringify(request)) throw new Error("different request");
    return found?.reference ?? null;
  });
  const uploadAttachment = vi.fn(async (request: AttachmentUploadRequest, payload: string) => {
    expect(uploads.list().some(item => item.request.uploadId === request.uploadId)).toBe(true);
    expect(atob(payload)).toBe("original");
    const reference = { name: request.name, mimeType: request.mimeType, path: `/app/attachments/${request.uploadId}/payload.txt`,
      integrity: { sha256: request.sha256, byteLength: request.byteLength } };
    saved.set(request.uploadId, { request, reference }); return reference;
  });
  const client = { uploadAttachment, attachmentUploadReceipt } as unknown as CoreClient;
  return { client, saved, uploadAttachment, attachmentUploadReceipt };
}
it("persists original metadata before upload, recovers a lost reply and reuses the same object", async () => {
  const f = fixture(), id = crypto.randomUUID(), save = f.uploadAttachment.getMockImplementation()!;
  f.uploadAttachment.mockImplementationOnce(async (...args) => { await save(...args); throw new Error("lost reply"); });
  const result = await uploads.prepare(file(), id, "text/plain", f.client);
  expect(result.reference.integrity?.byteLength).toBe(8);
  expect(uploads.list()).toEqual([{ schemaVersion: 1, request: result.request }]);
  expect(await uploads.prepare(file(), id, "text/plain", f.client)).toEqual(result);
  expect(f.uploadAttachment).toHaveBeenCalledTimes(1);
  expect(await uploads.check(uploads.list()[0], f.client)).toEqual(result.reference);
});
it("keeps a queryable intent when upload and receipt are unavailable without assuming failure", async () => {
  const f = fixture(), id = crypto.randomUUID();
  f.uploadAttachment.mockRejectedValueOnce(new Error("offline"));
  await expect(uploads.prepare(file(), id, "text/plain", f.client)).rejects.toThrow("offline");
  expect(uploads.list()[0].request.uploadId).toBe(id);
  expect(await uploads.check(uploads.list()[0], f.client)).toBeNull(); expect(f.uploadAttachment).toHaveBeenCalledTimes(1);
  await uploads.prepare(file(), id, "text/plain", f.client);
  expect(f.uploadAttachment.mock.calls[1]).toEqual(f.uploadAttachment.mock.calls[0]);
});
it("does not upload after a failed initial receipt query", async () => {
  const f = fixture(); f.attachmentUploadReceipt.mockRejectedValueOnce(new Error("offline"));
  await expect(uploads.prepare(file(), crypto.randomUUID(), "text/plain", f.client)).rejects.toThrow("offline");
  expect(f.uploadAttachment).not.toHaveBeenCalled(); expect(uploads.list()).toHaveLength(1);
});
it("blocks changed bytes under the original identity before another RPC", async () => {
  const f = fixture(), id = crypto.randomUUID(); await uploads.prepare(file(), id, "text/plain", f.client);
  f.attachmentUploadReceipt.mockClear();
  await expect(uploads.prepare(file("modified"), id, "text/plain", f.client)).rejects.toThrow("其他内容");
  expect(f.uploadAttachment).toHaveBeenCalledTimes(1); expect(f.attachmentUploadReceipt).not.toHaveBeenCalled();
});
it("does not upload when persistence fails", async () => {
  const f = fixture(); vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage full"); });
  await expect(uploads.prepare(file(), crypto.randomUUID(), "text/plain", f.client)).rejects.toThrow("storage full");
  expect(f.attachmentUploadReceipt).not.toHaveBeenCalled(); expect(f.uploadAttachment).not.toHaveBeenCalled();
});
it("rejects a mismatched receipt and retains original tracking", async () => {
  const f = fixture(); f.attachmentUploadReceipt.mockResolvedValue({ name: "other.txt", mimeType: "text/plain", path: "/wrong", integrity: { sha256: "0".repeat(64), byteLength: 8 } });
  await expect(uploads.prepare(file(), crypto.randomUUID(), "text/plain", f.client)).rejects.toThrow("不一致");
  expect(f.uploadAttachment).not.toHaveBeenCalled(); expect(uploads.list()).toHaveLength(1);
});
it("concurrent preparation of the same attachment publishes once and does not mix other records", async () => {
  const f = fixture(), id = crypto.randomUUID();
  const [first, second] = await Promise.all([uploads.prepare(file(), id, "text/plain", f.client), uploads.prepare(file(), id, "text/plain", f.client)]);
  expect(first).toEqual(second); expect(f.uploadAttachment).toHaveBeenCalledTimes(1);
  await uploads.prepare(file(), crypto.randomUUID(), "text/plain", f.client);
  uploads.forget(id); expect(uploads.list()).toHaveLength(1);
});
it("does not silently drop damaged records", () => {
  const id = crypto.randomUUID(); localStorage.setItem(`dogcodes:pending-attachment-upload:${id}`, "{}");
  expect(uploads.snapshot()).toContain("损坏"); expect(() => uploads.forget(id)).toThrow("损坏");
});
