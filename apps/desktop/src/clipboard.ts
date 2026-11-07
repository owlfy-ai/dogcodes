import { invoke, isTauri } from "@tauri-apps/api/core";

export interface ClipboardContent { text: string | null; image: File | null }

export const copyText = (text: string): Promise<void> => navigator.clipboard.writeText(text);

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
