import type { ArtifactChunk, ArtifactMetadata } from "@dogcodes/protocol";
import { translate as t } from "./i18n";

export function acceptArtifactChunk(metadata: ArtifactMetadata, current: Uint8Array, chunk: ArtifactChunk): Uint8Array {
  const end = current.length + chunk.bytes.length;
  if (chunk.metadata.artifactId !== metadata.artifactId
    || chunk.metadata.payloadSha256 !== metadata.payloadSha256
    || chunk.metadata.byteLength !== metadata.byteLength
    || chunk.offset !== current.length || chunk.bytes.length > 32768
    || end > metadata.byteLength || metadata.byteLength > 1048576
    || chunk.bytes.some((byte) => !Number.isInteger(byte) || byte < 0 || byte > 255)
    || chunk.nextOffset !== (end < metadata.byteLength ? end : null)
    || (end < metadata.byteLength && chunk.bytes.length === 0)) {
    throw new Error(t("artifactView.chunkMismatch"));
  }
  const bytes = new Uint8Array(end);
  bytes.set(current);
  bytes.set(chunk.bytes, current.length);
  return bytes;
}

export function artifactPreview(bytes: Uint8Array, complete: boolean, formatJson = false): { text: string; hexadecimal: boolean } {
  try {
    let text = new TextDecoder("utf-8", { fatal: true }).decode(bytes, { stream: !complete });
    if (complete && formatJson) {
      try { text = JSON.stringify(JSON.parse(text), null, 2); } catch { /* Preserve invalid JSON as text. */ }
    }
    return { text: artifactVisibleText(text), hexadecimal: false };
  } catch {
    const rows: string[] = [];
    for (let offset = 0; offset < Math.min(bytes.length, 256); offset += 16) {
      rows.push(`${offset.toString(16).padStart(8, "0")}  ${[...bytes.slice(offset, offset + 16)]
        .map((byte) => byte.toString(16).padStart(2, "0")).join(" ")}`);
    }
    return { text: rows.join("\n"), hexadecimal: true };
  }
}

export function artifactVisibleText(text: string): string {
  return text.replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/gu,
    (value) => `\\u${value.charCodeAt(0).toString(16).padStart(4, "0")}`);
}

export async function verifyArtifactBytes(bytes: Uint8Array, expectedHash: string): Promise<void> {
  const hash = await crypto.subtle.digest("SHA-256", new Uint8Array(bytes));
  const actual = [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  if (actual !== expectedHash) throw new Error(t("artifactView.hashMismatch"));
}
