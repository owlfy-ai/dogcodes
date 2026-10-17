import { createTwoFilesPatch } from "diff";
import { type DiffParseResult, type StructuredDiff, parseGitDiff } from "./diffView";
const header = (path: string) =>
  `diff --git ${JSON.stringify(`a/${path}`)} ${JSON.stringify(`b/${path}`)}\n`;
export function editPatch(path: string, before: string, after: string): string {
  const unified = createTwoFilesPatch(`a/${path}`, `b/${path}`, before, after);
  return header(path) + unified.slice(unified.indexOf("--- "));
}
export function reportedPatch(path: string, patch: string): string {
  if (patch.startsWith("diff --git ")) return patch;
  if (patch.startsWith("@@"))
    return `${header(path)}--- ${JSON.stringify(`a/${path}`)}\n+++ ${JSON.stringify(`b/${path}`)}\n${patch}`;
  const start = patch.indexOf("--- ");
  return start >= 0 ? header(path) + patch.slice(start) : patch;
}
function decode(file: { hex?: string } | null | undefined): string | null {
  if (file === null) return "";
  if (
    !file ||
    typeof file.hex !== "string" ||
    !/^(?:[a-f\d]{2})*$/i.test(file.hex)
  )
    return null;
  const bytes = Uint8Array.from(file.hex.match(/../g) ?? [], (h) =>
    parseInt(h, 16),
  );
  if (bytes.includes(0)) return null;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}
export function frozenDiff(data: {
  patch?: string;
  roots?: { workspacePath: string; changes: { patch?: string } }[];
  files: {
    path: string;
    before?: { hex?: string } | null;
    after?: { hex?: string } | null;
  }[];
}): DiffParseResult {
  if (data.roots?.length) {
    const diff: StructuredDiff = { files: [], additions: 0, deletions: 0, truncated: false, truncationReason: null };
    for (const root of data.roots) {
      if (!root.changes.patch?.startsWith("diff --git ")) continue;
      const parsed = parseGitDiff(new TextEncoder().encode(root.changes.patch));
      if (parsed.kind !== "structured") { diff.truncated = true; diff.truncationReason = parsed.reason; continue; }
      const full = (path: string) => path ? `${root.workspacePath.replace(/[\\\/]+$/, "")}/${path}` : "";
      diff.files.push(...parsed.diff.files.map(file => ({ ...file, oldPath: full(file.oldPath), newPath: full(file.newPath) })));
      diff.additions += parsed.diff.additions; diff.deletions += parsed.diff.deletions;
      if (parsed.diff.truncated) { diff.truncated = true; diff.truncationReason = parsed.diff.truncationReason; }
    }
    const captured = new Set(diff.files.flatMap(file => [file.oldPath, file.newPath]));
    const missing = data.files.filter(file => !captured.has(file.path));
    if (missing.length) {
      const fallback = frozenDiff({ files: missing });
      if (fallback.kind === "structured") { diff.files.push(...fallback.diff.files); diff.additions += fallback.diff.additions; diff.deletions += fallback.diff.deletions; if (fallback.diff.truncated) { diff.truncated = true; diff.truncationReason = fallback.diff.truncationReason; } }
    }
    return { kind: "structured", diff };
  }
  if (data.patch?.startsWith("diff --git "))
    return parseGitDiff(new TextEncoder().encode(data.patch));
  let patch = "";
  for (const file of data.files) {
    const before = decode(file.before),
      after = decode(file.after);
    if (before !== null && after !== null)
      patch += editPatch(file.path, before, after);
  }
  return parseGitDiff(new TextEncoder().encode(patch));
}
