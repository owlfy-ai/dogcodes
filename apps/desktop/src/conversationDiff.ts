import { createTwoFilesPatch } from "diff";
import { type DiffParseResult, type StructuredDiff, type DiffFile, parseGitDiff } from "./diffView";
import { translate as t } from "./i18n";
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
export interface CapturedVersion { hex?: string; sha256?: string; contentOmitted?: boolean; omissionReason?: string; contentArtifact?: import("@dogcodes/protocol").ArtifactMetadata }
export interface CapturedFile {
  workspacePath?: string; workspaceRelativePath?: string; relativePath?: string;
  path: string; status?: string; before?: CapturedVersion | null; after?: CapturedVersion | null;
  binary?: boolean; diffUnavailableReason?: string; additions?: number; deletions?: number;
}
const unavailable = (reason: string) => ({
  file_limit: t("historicalDiff.fileLimit"), snapshot_limit: t("historicalDiff.snapshotLimit"),
  binary: t("historicalDiff.binary"), not_saved: t("historicalDiff.notSaved"),
}[reason] ?? t("historicalDiff.notSaved"));
export function capturedFileDiff(file: CapturedFile): DiffFile {
  const reason = file.diffUnavailableReason ?? [file.before, file.after].find(v => v?.contentOmitted)?.omissionReason;
  const placeholder: DiffFile = {
    oldPath: file.path, newPath: file.path,
    change: file.status === "A" ? "added" : file.status === "D" ? "deleted" : "modified",
    oldMode: null, newMode: null, similarity: null, binary: file.binary ?? false,
    hunks: [], additions: file.additions ?? 0, deletions: file.deletions ?? 0,
  };
  if (reason || file.binary) return { ...placeholder, unavailableReason: unavailable(reason ?? "binary"), countsUnavailable: true };
  if (file.before?.contentArtifact || file.after?.contentArtifact) return { ...placeholder, pendingContent: true };
  const before = decode(file.before), after = decode(file.after);
  if (before === null || after === null) return { ...placeholder, unavailableReason: unavailable("not_saved"), countsUnavailable: true };
  if (before === after) return placeholder;
  // Bound CPU and render work for one file, without discarding its neighbours.
  const unified = createTwoFilesPatch(`a/${file.path}`, `b/${file.path}`, before, after, undefined, undefined, { timeout: 150 });
  if (unified === undefined) return { ...placeholder, unavailableReason: t("historicalDiff.parseLimit"), countsUnavailable: file.additions === undefined };
  const patch = header(file.path) + unified.slice(unified.indexOf("--- "));
  const parsed = parseGitDiff(new TextEncoder().encode(patch));
  if (parsed.kind === "structured" && !parsed.diff.truncated && parsed.diff.files[0]) return { ...parsed.diff.files[0], change: placeholder.change };
  return { ...placeholder, unavailableReason: parsed.kind === "fallback" ? parsed.reason : t("historicalDiff.parseLimit"), countsUnavailable: file.additions === undefined };
}
export function frozenDiff(data: {
  patch?: string;
  roots?: { workspacePath?: string; changes?: { patch?: string } }[];
  files: CapturedFile[];
}, selected?: ReadonlySet<string>): DiffParseResult {
  const diff: StructuredDiff = { files: [], additions: 0, deletions: 0, truncated: false, truncationReason: null };
  // Git patches already include rename/mode metadata. Filesystem snapshots are
  // independently parsed, so the global parser budget cannot hide later files.
  const patches = data.roots?.map(root => ({ path: root.workspacePath ?? "", patch: root.changes?.patch }))
    ?? [{ path: "", patch: data.patch }];
  for (const root of patches) {
    if (!root.patch?.startsWith("diff --git ")) continue;
    const parsed = parseGitDiff(new TextEncoder().encode(root.patch));
    if (parsed.kind !== "structured") continue;
    const full = (path: string) => root.path && path ? `${root.path.replace(/[\\/]+$/, "")}/${path}` : path;
    for (const file of parsed.diff.files) {
      const oldPath = full(file.oldPath), newPath = full(file.newPath);
      // For a rename with only one selected side, use that side's captured
      // version below instead of exposing the unrelated path.
      if (selected && ((oldPath && !selected.has(oldPath)) || (newPath && !selected.has(newPath)))) continue;
      diff.files.push({ ...file, oldPath, newPath });
    }
    if (parsed.diff.truncated) { diff.truncated = true; diff.truncationReason = parsed.diff.truncationReason; }
  }
  const captured = new Set(diff.files.flatMap(file => [file.oldPath, file.newPath]));
  diff.files.push(...data.files.filter(file => !captured.has(file.path)).map(capturedFileDiff));
  diff.additions = diff.files.reduce((sum, file) => sum + file.additions, 0);
  diff.deletions = diff.files.reduce((sum, file) => sum + file.deletions, 0);
  return { kind: "structured", diff };
}
