import type { TimelineItem } from "@dogcodes/protocol";
import { frozenDiff } from "./conversationDiff";
import type { TurnArtifact } from "./turnArtifactCache";

// Only completed write tools provide authorship. A terminal command or a read
// mentioning a path is not evidence that it changed that file.
const writeTools = new Set(["edit", "write", "multiedit", "apply_patch", "filechange", "file_change",
  "edit_file", "replace_in_file", "write_file", "write_to_file", "replace_file_content",
  "multi_replace_file_content", "delete_file", "move_file", "rename_file"]);
export function isWriteTool(name: string, category?: string) {
  return writeTools.has(name) || ["edit", "delete", "move"].includes(category ?? "");
}

const editsByItem = new WeakMap<TimelineItem, readonly string[]>();
function reportedWrites(item: TimelineItem): string[] {
  let a;
  try { a = JSON.parse(item.text); } catch { return []; }
  if (a?.type !== "tool" || !["completed", "succeeded", "success"].includes(a.status) || a.output?.success === false) return [];
  const name = typeof a.name === "string" ? a.name.toLowerCase() : "";
  if (!isWriteTool(name, a.category)) return [];
  const paths = new Set<string>();
  const add = (path: unknown) => { if (typeof path === "string" && path.length) paths.add(path); };
  if (a.output?.display?.kind === "file_diff") add(a.output.display.filePath);
  if (item.providerId === "zcode" && name === "write" && typeof a.output?.content === "string") {
    add(/^File created successfully at: ([^\r\n]+)(?:\r?\n)?$/.exec(a.output.content)?.[1]);
  }
  let input = a.input;
  if (typeof input === "string" && input.startsWith("{")) {
    try { input = JSON.parse(input); } catch { /* Only structured inputs supply paths. */ }
  }
  add(input?.file_path ?? input?.filePath ?? input?.path ?? input?.target_file ?? input?.TargetFile ?? input?.AbsolutePath);
  if (a.category === "move" || ["move_file", "rename_file"].includes(name)) {
    add(input?.source ?? input?.old_path); add(input?.destination ?? input?.new_path);
  }
  if (Array.isArray(a.changes)) for (const change of a.changes) {
    add(change?.path); add(change?.kind?.move_path ?? change?.kind?.movePath);
  }
  if (Array.isArray(a.content)) for (const block of a.content) {
    if (block?.type === "diff") add(block.path);
  }
  if (Array.isArray(a.locations)) for (const location of a.locations) add(location?.path);
  if (name === "apply_patch") {
    const patch = typeof input === "string" ? input : input?.patch;
    if (typeof patch === "string") for (const match of patch.matchAll(/^\*\*\* (?:Add File|Update File|Delete File|Move to): ([^\r\n]+)\r?$/gm)) add(match[1]);
  }
  return [...paths];
}

export function confirmedTurnPaths(items: readonly TimelineItem[]): Map<string, Set<string>> {
  const turns = new Map<string, Set<string>>();
  for (const item of items) {
    if (!item.turnId || item.kind !== "activity") continue;
    let paths = editsByItem.get(item);
    if (!paths) {
      paths = reportedWrites(item);
      editsByItem.set(item, paths);
    }
    if (!paths.length) continue;
    const turn = turns.get(item.turnId) ?? new Set<string>();
    paths.forEach(path => turn.add(path));
    turns.set(item.turnId, turn);
  }
  return turns;
}

const absolute = (path: string) => /^(?:\/|[A-Za-z]:[\\/])/.test(path);
function normalized(path: string, windows: boolean) {
  const parts: string[] = [];
  for (const part of (windows ? path.toLowerCase().replaceAll("\\", "/") : path).split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return parts.join("/");
}

function confirmedFiles(data: TurnArtifact["data"], paths: ReadonlySet<string>) {
  const primary = data.workspacePaths?.[0] ?? data.roots?.[0]?.workspacePath ?? "";
  const windows = /^[A-Za-z]:[\\/]|^\\\\/.test(primary);
  const resolve = (path: string, root: string) => {
    const full = absolute(path) || (windows && path.startsWith("\\\\")) ? path : root ? root + "/" + path : path;
    return (absolute(full) || (windows && full.startsWith("\\\\")) ? "/" : "") + normalized(full, windows);
  };
  const reported = new Set(Array.from(paths, path => resolve(path, primary)));
  return data.files.filter(file => {
    const path = file.workspaceRelativePath ?? file.relativePath ?? file.path;
    return reported.has(resolve(path, file.workspacePath ?? primary));
  });
}

export function buildTurnArtifact(data: TurnArtifact["data"], paths?: ReadonlySet<string>): TurnArtifact {
  const files = paths ? confirmedFiles(data, paths) : data.files;
  const subset = files.length < data.files.length;
  const selected = subset ? new Set(files.map(file => file.path)) : undefined;
  const parsed = frozenDiff({ ...data, files }, selected);
  return { data: subset ? { ...data, files, patch: undefined } : data,
    diff: parsed.kind === "structured" ? parsed.diff : null, expanded: false, subset };
}
