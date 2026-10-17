import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { ArtifactChunk, ArtifactMetadata } from "@dogcodes/protocol";
import { acceptArtifactChunk, artifactPreview, verifyArtifactBytes } from "./artifactView";

const metadata: ArtifactMetadata = { artifactId: `art_${"0".repeat(26)}`, mediaType: "application/vnd.dogcodes.git-diff.v1.patch",
  payloadSha256: "a".repeat(64), byteLength: 3, createdAt: "2026-09-03T00:00:00Z", redactionRulesetVersion: 0, redactionCount: 0 };

describe("Artifact bounded viewer", () => {
  it("joins exact sequential bytes and rejects version, offset, byte and EOF mismatches", () => {
    const first: ArtifactChunk = { metadata, offset: 0, bytes: [97, 255], nextOffset: 2 };
    const bytes = acceptArtifactChunk(metadata, new Uint8Array(), first);
    expect([...acceptArtifactChunk(metadata, bytes, { metadata, offset: 2, bytes: [98], nextOffset: null })]).toEqual([97, 255, 98]);
    for (const changed of [
      { ...first, offset: 1 }, { ...first, bytes: [-1, 255] }, { ...first, bytes: [256, 255] },
      { ...first, bytes: [1.5, 255] }, { ...first, bytes: [], nextOffset: 0 },
      { ...first, nextOffset: null }, { ...first, metadata: { ...metadata, payloadSha256: "b".repeat(64) } },
    ]) expect(() => acceptArtifactChunk(metadata, new Uint8Array(), changed)).toThrow();
    expect(() => acceptArtifactChunk({ ...metadata, byteLength: 1048577 }, new Uint8Array(), first)).toThrow();
  });

  it("handles UTF-8 chunk boundaries and displays invalid encodings as bounded hex", () => {
    const unicode = new TextEncoder().encode("你好");
    expect(artifactPreview(unicode.slice(0, 2), false)).toEqual({ text: "", hexadecimal: false });
    expect(artifactPreview(unicode, true)).toEqual({ text: "你好", hexadecimal: false });
    const invalid = new Uint8Array(1000).fill(255);
    const hex = artifactPreview(invalid, true);
    expect(hex.hexadecimal).toBe(true);
    expect(hex.text.split("\n")).toHaveLength(16);
    expect(hex.text).not.toContain("�");
    expect(invalid[0]).toBe(255);
  });

  it("renders hostile HTML and escape sequences only as inert visible text", () => {
    const raw = '<script>alert(1)</script><img src="https://example.invalid/track">\x1b[2J\u202e';
    const preview = artifactPreview(new TextEncoder().encode(raw), true);
    const rendered = renderToStaticMarkup(createElement("pre", null, preview.text));
    expect(rendered).toContain("&lt;script&gt;");
    expect(rendered).not.toContain("<script>");
    expect(rendered).not.toContain("<img");
    expect(rendered).toContain("\\u001b[2J\\u202e");
  });

  it("checks the full byte hash before enabling export", async () => {
    await expect(verifyArtifactBytes(new Uint8Array(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")).resolves.toBeUndefined();
    await expect(verifyArtifactBytes(new Uint8Array([255]), "0".repeat(64))).rejects.toThrow("禁止导出");
  });

  it("formats complete JSON for reading without changing the export bytes", () => {
    const bytes = new TextEncoder().encode('{"kind":"handoff","pending":["review"]}');
    expect(artifactPreview(bytes, true, true).text).toContain('\n  "pending": [\n');
    expect(new TextDecoder().decode(bytes)).toBe('{"kind":"handoff","pending":["review"]}');
    expect(artifactPreview(bytes.slice(0, 4), false, true).text).toBe('{"ki');
  });
});
