// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { AttachmentUploadRequest } from "@dogcodes/protocol";
import { attachmentUploads as uploads } from "./attachmentUploads";
import { AttachmentUploadRecovery } from "./AttachmentUploadRecovery";
let root: Root, host: HTMLDivElement;
const request: AttachmentUploadRequest = { uploadId: "11111111-1111-4111-8111-111111111111", name: "saved.txt", mimeType: "text/plain", byteLength: 3, sha256: "a".repeat(64) };
beforeEach(() => { localStorage.clear(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); uploads.remember(request); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const render = (ids: string[] = [], busy = false) => act(async () => root.render(<AttachmentUploadRecovery busy={busy} attachedIds={ids}/>));

it("shows nothing and drops an upload record no draft uses", async () => {
  await render();
  expect(host.textContent).toBe(""); expect(uploads.list()).toEqual([]);
});
it("keeps records of attachments still in the draft or being sent", async () => {
  await render([request.uploadId]); expect(uploads.list()).toHaveLength(1);
  await render([], true); expect(uploads.list()).toHaveLength(1);
  await render([], false); expect(uploads.list()).toEqual([]);
  expect(host.textContent).toBe("");
});
