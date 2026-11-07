import { invoke, isTauri } from "@tauri-apps/api/core";

export interface ClipboardContent { text: string | null; image: File | null }

export const copyText = (text: string): Promise<void> => navigator.clipboard.writeText(text);

/** WebKit allows a clipboard write only during the click, so text that must first be
 * fetched goes in as a promise rather than being written once it arrives. */
export function copyPendingText(text: Promise<string>): Promise<void> {
  if (typeof ClipboardItem === "undefined") return text.then(copyText);
  return navigator.clipboard.write([new ClipboardItem({ "text/plain": text.then(value => new Blob([value], { type: "text/plain" })) })]);
}

function pngFile(base64: string): File {
  const bytes = Uint8Array.from(atob(base64), character => character.charCodeAt(0));
  return new File([bytes], "clipboard.png", { type: "image/png" });
}

/** For the app's own Paste item. The web view's API puts up a system prompt to confirm
 * each read, and doesn't reach images, so the desktop app reads it natively. */
export async function readClipboard(includeImage = false): Promise<ClipboardContent> {
  if (!isTauri()) return { text: await navigator.clipboard.readText() || null, image: null };
  const { text, image } = await invoke<{ text: string | null; image: string | null }>("read_clipboard", { includeImage });
  return { text, image: image ? pngFile(image) : null };
}
