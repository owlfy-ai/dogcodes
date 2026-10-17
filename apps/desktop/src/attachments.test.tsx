import { webcrypto, createHash } from "node:crypto";
import { attachmentUploads } from "./attachmentUploads";
// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, expect, it, vi } from "vitest";
import { AttachmentPreviews, useAttachments } from "./attachments";

beforeEach(() => {
  localStorage.clear();
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
});

it("opens full images, dismisses without removing them, and restores thumbnail focus", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let state!: ReturnType<typeof useAttachments>;
  function Harness() { state = useAttachments(); return <AttachmentPreviews items={state.items} remove={state.remove} disabled={false} />; }
  const host = document.createElement("div"); document.body.append(host);
  const root = createRoot(host);
  try {
    await act(async () => root.render(<Harness />));
    await act(async () => state.add([new File(["image"], "clipboard.png", { type: "image/png" })]));
    const trigger = host.querySelector<HTMLButtonElement>(".attachment-preview-trigger")!;
    for (const dismissal of ["button", "backdrop", "cancel"]) {
      trigger.focus();
      await act(async () => trigger.click());
      const dialog = document.querySelector<HTMLDialogElement>(".attachment-preview-dialog")!;
      expect(dialog.open).toBe(true);
      expect(document.activeElement).toBe(dialog);
      expect(dialog.querySelector("img")?.src).toBe(trigger.querySelector("img")?.src);
      await act(async () => dialog.querySelector("img")!.click());
      expect(dialog.isConnected).toBe(true);
      await act(async () => {
        if (dismissal === "button") dialog.querySelector<HTMLButtonElement>('[aria-label="关闭图片预览"]')!.click();
        else if (dismissal === "backdrop") dialog.click();
        else dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
      });
      expect(document.querySelector(".attachment-preview-dialog")).toBeNull();
      expect(document.activeElement).toBe(trigger);
      expect(state.items).toHaveLength(1);
    }
    await act(async () => host.querySelector<HTMLButtonElement>(".attachment-remove")!.click());
    expect(state.items).toHaveLength(0);
    expect(document.querySelector(".attachment-preview-dialog")).toBeNull();
  } finally { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); }
});

it("allows preview while sending and closes the dialog when attachments are cleared", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const host = document.createElement("div"); document.body.append(host);
  const root = createRoot(host);
  const remove = vi.fn();
  try {
    await act(async () => root.render(<AttachmentPreviews items={[{ id: "image", name: "photo.png", preview: "data:image/png;base64,aW1hZ2U=", file: new File(["image"], "photo.png") }]} remove={remove} disabled />));
    expect(host.querySelector<HTMLButtonElement>(".attachment-remove")!.disabled).toBe(true);
    await act(async () => host.querySelector<HTMLButtonElement>(".attachment-preview-trigger")!.click());
    expect(document.querySelector("dialog[open]")).not.toBeNull();
    await act(async () => root.render(<AttachmentPreviews items={[]} remove={remove} disabled />));
    expect(document.querySelector(".attachment-preview-dialog")).toBeNull();
    expect(remove).not.toHaveBeenCalled();
  } finally { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); }
});

it("renders image thumbnails and clears pending attachments", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let state!: ReturnType<typeof useAttachments>;
  function Harness() { state = useAttachments(); return <AttachmentPreviews items={state.items} remove={state.remove} disabled={false} />; }
  const host = document.createElement("div"); const root = createRoot(host);
  try {
    await act(async () => root.render(<Harness />));
    await act(async () => state.add([new File(["image"], "photo.png", { type: "image/png" })]));
    expect(host.querySelector("img")?.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
    expect(host.querySelector("img")?.alt).toBe("photo.png");
    await act(async () => state.clear());
    expect(host.querySelector("img")).toBeNull();
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});

it("infers clipboard image MIME and sends independent attachment metadata", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("__TAURI_INTERNALS__", {});
  const { invoke } = await import("@tauri-apps/api/core");
  vi.stubGlobal("crypto", webcrypto);
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "core_attachment_upload_receipt") return null;
    const { request } = args as { request: import("@dogcodes/protocol").AttachmentUploadRequest };
    return { name: request.name, mimeType: request.mimeType, path: "/app/attachments/clipboard.png", integrity: { sha256: request.sha256, byteLength: request.byteLength } };
  });
  let state!: ReturnType<typeof useAttachments>;
  function Harness() { state = useAttachments(); return null; }
  const root = createRoot(document.createElement("div"));
  try {
    await act(async () => root.render(<Harness />));
    const file = new File(["png"], "clipboard.png");
    Object.defineProperty(file, "arrayBuffer", { value: async () => new TextEncoder().encode("png").buffer });
    await act(async () => state.add([file]));
    expect(await state.prepare()).toEqual([{ name: "clipboard.png", path: "/app/attachments/clipboard.png", mimeType: "image/png", integrity: { sha256: createHash("sha256").update("png").digest("hex"), byteLength: 3 } }]);
    expect(state.items).toHaveLength(1);
    expect(attachmentUploads.list()).toHaveLength(1);
    await act(async () => state.consume(state.items.map(item => item.id)));
    expect(attachmentUploads.list()).toEqual([]);
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

it("switches only between images, respects gallery boundaries, and downloads the selected image", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const host = document.createElement("div"); document.body.append(host);
  const root = createRoot(host);
  const items = [
    { id: "first", name: "first.png", preview: "data:image/png;base64,YQ==", file: new File(["a"], "first.png") },
    { id: "text", name: "notes.txt", file: new File(["notes"], "notes.txt") },
    { id: "second", name: "second.png", preview: "data:image/png;base64,Yg==", file: new File(["b"], "second.png") },
  ];
  const downloads: { href: string; name: string }[] = [];
  const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) { downloads.push({ href: this.href, name: this.download }); });
  try {
    await act(async () => root.render(<AttachmentPreviews items={items} remove={vi.fn()} disabled={false} />));
    await act(async () => host.querySelectorAll<HTMLButtonElement>(".attachment-preview-trigger")[1].click());
    const dialog = document.querySelector<HTMLDialogElement>(".attachment-preview-dialog")!;
    const previous = dialog.querySelector<HTMLButtonElement>('[aria-label="上一张图片"]')!;
    const next = dialog.querySelector<HTMLButtonElement>('[aria-label="下一张图片"]')!;
    expect(dialog.querySelector("header")).toBeNull();
    expect(dialog.querySelector("img")?.alt).toBe("second.png");
    expect(next.disabled).toBe(true);
    await act(async () => previous.click());
    expect(dialog.querySelector("img")?.alt).toBe("first.png");
    expect(previous.disabled).toBe(true);
    expect(document.activeElement).toBe(dialog);
    await act(async () => dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true })));
    expect(dialog.querySelector("img")?.alt).toBe("first.png");
    await act(async () => dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true })));
    expect(dialog.querySelector(".attachment-preview-counter")?.textContent).toBe("2 / 2");
    await act(async () => dialog.querySelector<HTMLButtonElement>('[aria-label="下载图片"]')!.click());
    expect(downloads).toEqual([{ href: items[2].preview, name: "second.png" }]);
    expect(dialog.open).toBe(true);
  } finally { click.mockRestore(); await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); }
});

it("exports the original image bytes on desktop and shows download errors without closing", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("__TAURI_INTERNALS__", {});
  const { invoke } = await import("@tauri-apps/api/core");
  vi.mocked(invoke).mockResolvedValue(false);
  const file = new File(["image"], "photo.png");
  Object.defineProperty(file, "arrayBuffer", { value: async () => new Uint8Array([1, 2, 3]).buffer });
  const host = document.createElement("div"); const root = createRoot(host);
  try {
    await act(async () => root.render(<AttachmentPreviews items={[{ id: "one", name: file.name, file, preview: "data:image/png;base64,YQ==" }]} remove={vi.fn()} disabled={false} />));
    await act(async () => host.querySelector<HTMLButtonElement>(".attachment-preview-trigger")!.click());
    const dialog = document.querySelector<HTMLDialogElement>(".attachment-preview-dialog")!;
    expect(dialog.querySelector('[aria-label="下一张图片"]')).toBeNull();
    const download = dialog.querySelector<HTMLButtonElement>('[aria-label="下载图片"]')!;
    await act(async () => download.click());
    expect(invoke).toHaveBeenCalledWith("core_export_attachment", { name: "photo.png", bytes: [1, 2, 3] });
    expect(dialog.querySelector('[role="alert"]')).toBeNull();
    vi.mocked(invoke).mockRejectedValueOnce(new Error("write failed"));
    await act(async () => download.click());
    expect(dialog.querySelector('[role="alert"]')?.textContent).toContain("图片下载失败");
    expect(dialog.open).toBe(true);
    expect(download.disabled).toBe(false);
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});

it("restores sent image previews and file cards without showing internal paths", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const createUrl = vi.fn().mockReturnValue("blob:attachment-preview");
  const revokeUrl = vi.fn();
  vi.stubGlobal("URL", { createObjectURL: createUrl, revokeObjectURL: revokeUrl });
  const { invoke } = await import("@tauri-apps/api/core");
  vi.mocked(invoke).mockResolvedValue([137, 80, 78, 71]);
  const { SentAttachments } = await import("./attachments");
  const host = document.createElement("div"); const root = createRoot(host);
  try {
    await act(async () => root.render(<SentAttachments items={[
      {name: "截图.png", path: "/private/app/attachments/a.png", mimeType: "image/png"},
      {name: "说明.txt", path: "/private/app/attachments/a.txt", mimeType: "text/plain"},
      {name: "第二张.png", path: "/private/app/attachments/b.png", mimeType: "image/png"},
    ]} />));
    expect(host.querySelector("img")?.src).toBe("blob:attachment-preview");
    expect(host.textContent).toContain("说明.txt");
    expect(host.textContent).not.toContain("/private/");
    expect(invoke).toHaveBeenCalledWith("core_read_attachment", {path: "/private/app/attachments/a.png"});
    await act(async () => host.querySelector<HTMLButtonElement>(".attachment-preview-trigger")!.click());
    expect(document.querySelector(".attachment-preview-dialog img")?.getAttribute("src")).toBe("blob:attachment-preview");
    vi.mocked(invoke).mockRejectedValueOnce(new Error("missing image"));
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="下一张图片"]')!.click());
    expect(document.querySelector(".attachment-preview-dialog img")).toBeNull();
    expect(document.querySelector(".attachment-preview-status")?.textContent).toBe("图片已不可读取");
    expect(document.querySelector<HTMLButtonElement>('[aria-label="下载图片"]')!.disabled).toBe(true);
    expect(document.querySelector(".attachment-preview-counter")?.textContent).toBe("2 / 2");
    await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="上一张图片"]')!.click());
    expect(document.querySelector(".attachment-preview-dialog img")?.getAttribute("alt")).toBe("截图.png");
  } finally {
    await act(async () => root.unmount());
    expect(revokeUrl).toHaveBeenCalledWith("blob:attachment-preview");
    vi.unstubAllGlobals();
  }
});

it("consumes only the sent attachment IDs and retains files selected later", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let state!: ReturnType<typeof useAttachments>;
  function Harness() { state = useAttachments(); return null; }
  const root = createRoot(document.createElement("div"));
  try {
    await act(async () => root.render(<Harness />));
    await act(async () => state.add([new File(["old"], "notes.txt")]));
    const original = state.items.map(item => item.id);
    await act(async () => state.clear());
    await act(async () => state.add([new File(["new"], "notes.txt")]));
    await act(async () => state.consume(original));
    expect(state.items.map(item => item.file?.size)).toEqual([3]);
    expect(state.items[0].id).not.toBe(original[0]);
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});

it("can send a recovered attachment without the original browser File, and rechecks its receipt", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.stubGlobal("__TAURI_INTERNALS__", {});
  const request = { uploadId: "11111111-1111-4111-8111-111111111111", name: "recovered.txt", mimeType: "text/plain", sha256: "a".repeat(64), byteLength: 5 };
  const reference = { name: request.name, path: "/app/recovered.txt", mimeType: request.mimeType, integrity: { sha256: request.sha256, byteLength: request.byteLength } };
  const receipt = vi.fn(async () => reference), upload = vi.fn();
  const client = { attachmentUploadReceipt: receipt, uploadAttachment: upload } as unknown as import("@dogcodes/protocol").CoreClient;
  let state!: ReturnType<typeof useAttachments>;
  function Harness() { state = useAttachments(client); return null; }
  const root = createRoot(document.createElement("div"));
  try {
    await act(async () => root.render(<Harness/>));
    await act(async () => state.recover(request, reference));
    expect(await state.prepare()).toEqual([reference]); expect(receipt).toHaveBeenCalledWith(request); expect(upload).not.toHaveBeenCalled();
    receipt.mockRejectedValueOnce(new Error("content changed")); await expect(state.prepare()).rejects.toThrow("content changed");
    expect(state.items).toHaveLength(1); expect(attachmentUploads.list()).toHaveLength(1);
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});
