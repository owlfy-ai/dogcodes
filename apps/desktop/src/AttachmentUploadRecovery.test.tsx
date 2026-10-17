// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { AttachmentUploadRequest, CoreClient, PromptAttachment } from "@dogcodes/protocol";
import { attachmentUploads as uploads } from "./attachmentUploads";
import { AttachmentUploadRecovery } from "./AttachmentUploadRecovery";
let root: Root, host: HTMLDivElement;
const request: AttachmentUploadRequest = { uploadId: "11111111-1111-4111-8111-111111111111", name: "saved.txt", mimeType: "text/plain", byteLength: 3, sha256: "a".repeat(64) };
const reference: PromptAttachment = { name: request.name, mimeType: request.mimeType, path: "/app/attachments/saved.txt", integrity: { sha256: request.sha256, byteLength: request.byteLength } };
beforeEach(() => { localStorage.clear(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); uploads.remember(request); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const click = async (label: string) => act(async () => { [...host.querySelectorAll("button")].find(button => button.textContent === label)!.click(); });
function fixture() {
  const attachmentUploadReceipt = vi.fn(async (): Promise<PromptAttachment | null> => reference), uploadAttachment = vi.fn(), onRecover = vi.fn();
  const client = { attachmentUploadReceipt, uploadAttachment } as unknown as CoreClient;
  const render = (ids: string[] = [], busy = false) => act(async () => root.render(<AttachmentUploadRecovery client={client} busy={busy} attachedIds={ids} onRecover={onRecover}/>));
  return { attachmentUploadReceipt, uploadAttachment, onRecover, render };
}
it("discovers a completed import after reopening and only attaches after explicit selection", async () => {
  const f = fixture(); await f.render(); await act(async () => root.unmount()); root = createRoot(host); await f.render();
  expect(f.attachmentUploadReceipt).not.toHaveBeenCalled(); expect(f.uploadAttachment).not.toHaveBeenCalled();
  await click("核实附件导入"); expect(f.attachmentUploadReceipt).toHaveBeenCalledExactlyOnceWith(request); expect(f.onRecover).not.toHaveBeenCalled();
  await click("添加到当前草稿"); expect(f.onRecover).toHaveBeenCalledWith(request, reference);
  expect(uploads.list()).toHaveLength(1); // Ownership transfers only when a send intent is durable.
  await f.render([request.uploadId]); expect(host.querySelector('[aria-label="待核实的附件导入"]')).toBeNull();
});
it("keeps missing or failed imports queryable without uploading or sending", async () => {
  const f = fixture(); f.attachmentUploadReceipt.mockResolvedValue(null); await f.render(); await click("核实附件导入");
  expect(host.textContent).toContain("重新选择原文件"); expect(f.uploadAttachment).not.toHaveBeenCalled(); expect(f.onRecover).not.toHaveBeenCalled();
  expect(uploads.list()).toHaveLength(1);
  f.attachmentUploadReceipt.mockRejectedValue(new Error("offline")); await click("核实附件导入"); expect(host.textContent).toContain("暂时无法核实");
});
it("hides local tracking explicitly without deleting server content", async () => {
  const f = fixture(); await f.render([], true); expect(host.textContent).toBe(""); await f.render();
  await click("隐藏导入记录"); expect(uploads.list()).toEqual([]); expect(f.attachmentUploadReceipt).not.toHaveBeenCalled(); expect(f.uploadAttachment).not.toHaveBeenCalled();
});
