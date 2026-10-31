import { attachmentUploads } from "./attachmentUploads";
import type { AttachmentUploadRequest, CoreClient, PromptAttachment } from "@dogcodes/protocol";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

export interface Attachment { id: string; name: string; preview?: string; file?: File; path?: string; stored?: PromptAttachment; upload?: AttachmentUploadRequest }
const MAX_SIZE = 25 * 1024 * 1024;
export function attachmentMime(file: Pick<File, "type" | "name">): string {
  if (file.type) return file.type;
  const extension = file.name.split(".").pop()?.toLowerCase() ?? "";
  return ({ png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", heic: "image/heic", pdf: "application/pdf", txt: "text/plain" } as Record<string, string>)[extension] ?? "application/octet-stream";
}
function preview(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error(t("attachments.readFailed", { name: file.name })));
    reader.readAsDataURL(file);
  });
}
export function useAttachments(client?: CoreClient) {
  const [items, setItems] = useState<Attachment[]>([]);
  const [pending, setPending] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const clear = useCallback(() => { generation.current++; setItems([]); setError(null); }, []);
  const consume = useCallback((ids: string[]) => {
    ids.forEach(attachmentUploads.forget);
    const sent = new Set(ids);
    setItems(previous => previous.filter(item => !sent.has(item.id)));
  }, []);
  const add = async (files: File[]) => {
    const epoch = generation.current;
    setPending(n => n + 1);
    setError(null);
    try {
      for (const file of files) {
        if (file.size > MAX_SIZE) throw new Error(t("attachments.tooLarge", { name: file.name }));
        const item: Attachment = { id: crypto.randomUUID(), name: file.name, file,
          preview: attachmentMime(file).startsWith("image/") ? await preview(file) : undefined };
        if (epoch === generation.current) setItems(previous => [...previous, item]);
      }
    } catch (cause) { if (epoch === generation.current) setError(userFacingError(cause, t("attachments.addFailed"))); }
    finally { setPending(n => n - 1); }
  };
  const recover = useCallback((upload: AttachmentUploadRequest, stored: PromptAttachment) => {
    setItems(previous => previous.some(item => item.id === upload.uploadId) ? previous : [...previous,
      { id: upload.uploadId, name: upload.name, path: stored.path, upload, stored }]);
  }, []);
  const prepare = async () => {
    if (items.length > 16 || items.reduce((sum, item) => sum + (item.file?.size ?? item.stored?.integrity?.byteLength ?? 0), 0) > MAX_SIZE) throw new Error(t("attachments.limit"));
    if (items.length && !("__TAURI_INTERNALS__" in window)) throw new Error(t("attachments.desktopOnly"));
    const prepared: PromptAttachment[] = [];
    for (const item of items) {
      if (item.file) {
        const result = await attachmentUploads.prepare(item.file, item.id, attachmentMime(item.file), client);
        item.upload = result.request; item.stored = result.reference; item.path = result.reference.path;
      } else if (item.upload) {
        attachmentUploads.remember(item.upload);
        const stored = await attachmentUploads.check({ schemaVersion: 1, request: item.upload }, client);
        if (!stored) throw new Error(t("attachments.uploadUnconfirmed"));
        item.stored = stored; item.path = stored.path;
      }
      if (!item.stored) throw new Error(t("attachments.unreadable"));
      prepared.push(structuredClone(item.stored));
    }
    return prepared;
  };
  return { items, pending, error, add, clear, consume, recover, prepare, remove: (id: string) => setItems(previous => previous.filter(item => item.id !== id)) };

}
interface GalleryImage { id: string; name: string; src?: string; file?: File; path?: string; mimeType?: string }

function useImageSource(image: GalleryImage): { src?: string; unavailable?: boolean } | undefined {
  const [loaded, setLoaded] = useState<{ path: string; src?: string; unavailable?: boolean }>();
  useEffect(() => {
    if (image.src || !image.path) return;
    const path = image.path;
    let disposed = false;
    let url: string | undefined;
    invoke<number[]>("core_read_attachment", { path }).then(bytes => {
      if (disposed) return;
      url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: image.mimeType }));
      setLoaded({ path, src: url });
    }).catch(() => { if (!disposed) setLoaded({ path, unavailable: true }); });
    return () => { disposed = true; if (url) URL.revokeObjectURL(url); };
  }, [image.src, image.path, image.mimeType]);
  return image.src ? { src: image.src } : loaded?.path === image.path ? loaded : undefined;
}

function ImagePreviewDialog({ images, selectedId, onSelect, onClose }: {
  images: GalleryImage[]; selectedId: string; onSelect: (id: string) => void; onClose: () => void;
}) {
  const index = images.findIndex(image => image.id === selectedId);
  const image = images[index];
  const source = useImageSource(image);
  const dialog = useRef<HTMLDialogElement>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadError, setDownloadError] = useState<string>();
  useEffect(() => {
    const element = dialog.current!;
    const previousFocus = document.activeElement;
    element.showModal();
    element.focus({ preventScroll: true });
    return () => {
      element.close();
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus();
    };
  }, []);
  useEffect(() => setDownloadError(undefined), [selectedId]);
  const select = (offset: number) => {
    const next = images[index + offset];
    if (next) {
      onSelect(next.id);
      dialog.current?.focus({ preventScroll: true });
    }
  };
  const download = async () => {
    if (!source?.src || downloading) return;
    dialog.current?.focus({ preventScroll: true });
    setDownloading(true); setDownloadError(undefined);
    try {
      if ("__TAURI_INTERNALS__" in window) {
        const bytes = image.file ? Array.from(new Uint8Array(await image.file.arrayBuffer()))
          : await invoke<number[]>("core_read_attachment", { path: image.path });
        await invoke("core_export_attachment", { name: image.name, bytes });
      } else {
        const link = document.createElement("a");
        link.href = source.src; link.download = image.name;
        document.body.append(link); link.click(); link.remove();
      }
    } catch { setDownloadError(t("attachments.downloadFailed")); }
    finally { setDownloading(false); }
  };
  return createPortal(<dialog ref={dialog} tabIndex={-1} className="attachment-preview-dialog" aria-label={t("attachments.previewLabel", { name: image.name })}
    onCancel={event => { event.preventDefault(); onClose(); }}
    onClick={event => { event.stopPropagation(); if (event.target === event.currentTarget) onClose(); }}
    onKeyDown={event => {
      event.stopPropagation();
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault(); select(event.key === "ArrowLeft" ? -1 : 1);
      }
    }}>
    <div className="attachment-preview-actions">
      <button type="button" onClick={() => void download()} disabled={!source?.src || downloading} aria-label={t("attachments.download")} data-tooltip={t("attachments.download")}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v12m-4-4 4 4 4-4M5 15v5h14v-5" /></svg>
      </button>
      <button type="button" onClick={onClose} aria-label={t("attachments.closePreview")} data-tooltip={t("attachments.closePreviewTooltip")}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18" /></svg>
      </button>
    </div>
    {source?.src ? <img className="attachment-preview-full-image" src={source.src} alt={image.name} draggable={false} />
      : <span className="attachment-preview-status" role="status">{t(source?.unavailable ? "attachments.imageUnavailable" : "attachments.imageLoading")}</span>}
    {images.length > 1 && <>
      <button type="button" className="attachment-preview-previous" disabled={index === 0} onClick={() => select(-1)} aria-label={t("attachments.previous")}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M19 12H5m6-6-6 6 6 6" /></svg>
      </button>
      <button type="button" className="attachment-preview-next" disabled={index === images.length - 1} onClick={() => select(1)} aria-label={t("attachments.next")}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14m-6-6 6 6-6 6" /></svg>
      </button>
      <span className="attachment-preview-counter" role="status" aria-label={t("attachments.counter", { index: index + 1, total: images.length })}>{index + 1} / {images.length}</span>
    </>}
    {downloadError && <p className="attachment-preview-error" role="alert">{downloadError}</p>}
  </dialog>, document.body);
}

function AttachmentImage({ image, onOpen }: { image: GalleryImage; onOpen: () => void }) {
  const source = useImageSource(image);
  return source?.src ? <button type="button" className="attachment-preview-trigger" aria-label={t("attachments.zoom", { name: image.name })} aria-haspopup="dialog" onClick={onOpen}>
    <img src={source.src} alt={image.name} />
  </button> : <span>{t(source?.unavailable ? "attachments.imageUnavailable" : "attachments.imageLoading")} · {image.name}</span>;
}

export function AttachmentPreviews({ items, remove, disabled }: { items: Attachment[]; remove: (id: string) => void; disabled: boolean }) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const images = items.filter(item => item.preview || item.stored?.mimeType.startsWith("image/")).map(item => ({ ...item, src: item.preview, mimeType: item.stored?.mimeType }));
  if (!items.length) return null;
  return <>
    <div className="attachment-list" aria-label={t("attachments.pending")}>{items.map(item => {
      const image = !!item.preview || !!item.stored?.mimeType.startsWith("image/");
      const extension = item.name.includes(".") ? item.name.split(".").pop()!.toUpperCase() : t("attachments.file");
      return <div key={item.id} className={`attachment-card ${image ? "attachment-image" : "attachment-file"}`} data-tooltip={item.name}>
        {image ? <AttachmentImage image={{ ...item, src: item.preview, mimeType: item.stored?.mimeType }} onOpen={() => setSelectedId(item.id)} /> : <><span className="attachment-icon" aria-hidden="true">▤</span><span className="attachment-details"><strong>{item.name}</strong><small>{extension}</small></span></>}
        <button type="button" className="attachment-remove" disabled={disabled} onClick={() => remove(item.id)} aria-label={t("attachments.remove", { name: item.name })}>×</button>
      </div>;
    })}</div>
    {selectedId && images.some(image => image.id === selectedId) && <ImagePreviewDialog images={images} selectedId={selectedId} onSelect={setSelectedId} onClose={() => setSelectedId(null)} />}
  </>;
}

export function SentAttachments({ items }: { items: PromptAttachment[] }) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const entries = items.map((item, index) => ({ ...item, id: `${item.path}:${index}` }));
  const images = entries.filter(item => item.mimeType.startsWith("image/"));
  if (!items.length) return null;
  return <>
    <div className="sent-attachments" aria-label={t("attachments.sent")}>{entries.map(item => <div className="sent-attachment" key={item.id} data-tooltip={item.name}>
      {item.mimeType.startsWith("image/") ? <AttachmentImage image={item} onOpen={() => setSelectedId(item.id)} /> : <><span aria-hidden="true">▤</span><span>{item.name}</span></>}
    </div>)}</div>
    {selectedId && images.some(image => image.id === selectedId) && <ImagePreviewDialog images={images} selectedId={selectedId} onSelect={setSelectedId} onClose={() => setSelectedId(null)} />}
  </>;
}
