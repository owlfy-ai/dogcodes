export type DiffLineKind = "context" | "addition" | "deletion" | "no_newline";

export interface DiffLine {
  kind: DiffLineKind;
  oldLine: number | null;
  newLine: number | null;
  text: string;
}

export interface DiffHunk {
  header: string;
  section: string | null;
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
  lines: DiffLine[];
}

export interface DiffFile {
  oldPath: string;
  newPath: string;
  change: "modified" | "added" | "deleted" | "renamed" | "copied" | "binary";
  oldMode: string | null;
  newMode: string | null;
  similarity: string | null;
  binary: boolean;
  hunks: DiffHunk[];
  additions: number;
  deletions: number;
}

export interface StructuredDiff {
  files: DiffFile[];
  additions: number;
  deletions: number;
  truncated: boolean;
  truncationReason: string | null;
}

export type DiffParseResult =
  | { kind: "structured"; diff: StructuredDiff }
  | { kind: "fallback"; reason: string };

const MAX_FILES = 500;
const MAX_HUNKS = 2_000;
const MAX_LINES = 10_000;

interface MutableHunk extends DiffHunk {
  seenOld: number;
  seenNew: number;
}

interface MutableFile extends DiffFile {
  currentHunk: MutableHunk | null;
  binaryPayload: boolean;
  movementFrom: boolean;
  movementTo: boolean;
}

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(?: ?(.*))?$/;

export function parseGitDiff(bytes: Uint8Array): DiffParseResult {
  if (bytes.length > 1_048_576) return { kind: "fallback", reason: "Patch 超过 1 MiB 解析预算，结构化视图不可用。" };
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return { kind: "fallback", reason: "Patch 不是有效 UTF-8，结构化视图不可用；请使用十六进制预览或导出原始字节。" };
  }
  if (text.length === 0) return { kind: "structured", diff: emptyDiff() };
  if (!text.endsWith("\n")) return { kind: "fallback", reason: "Patch 末尾不完整；已降级为原始内容视图。" };

  const files: DiffFile[] = [];
  let current: MutableFile | null = null;
  let additions = 0;
  let deletions = 0;
  let hunkCount = 0;
  let lineCount = 0;
  let truncated = false;
  let truncationReason: string | null = null;

  const fail = (reason: string): DiffParseResult => ({
    kind: "fallback",
    reason: `Patch 格式无效或暂不支持（${reason}）；已降级为原始内容视图。`,
  });

  const finishHunk = (): DiffParseResult | null => {
    const hunk = current?.currentHunk;
    if (!hunk) return null;
    if (!truncated && (hunk.seenOld !== hunk.oldCount || hunk.seenNew !== hunk.newCount)) {
      return fail("hunk 行数与头部声明不一致");
    }
    current!.currentHunk = null;
    return null;
  };

  const finishFile = (): DiffParseResult | null => {
    const invalid = finishHunk();
    if (invalid) return invalid;
    if (!current) return null;
    if (!current.binary && current.hunks.length === 0 && current.change === "modified"
      && (current.oldMode === current.newMode || !current.oldMode || !current.newMode)) return fail("文件头之后没有变更内容或模式信息");
    if ((current.change === "renamed" || current.change === "copied") && (!current.movementFrom || !current.movementTo)) return fail("重命名或复制路径不完整");
    const { currentHunk: _hunk, binaryPayload: _payload, movementFrom: _from, movementTo: _to, ...file } = current;
    files.push(file);
    current = null;
    return null;
  };

  const lines = text.split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]!;
    if (index === lines.length - 1 && line === "") break;

    if (line.startsWith("diff --git ")) {
      const invalid = finishFile();
      if (invalid) return invalid;
      if (files.length >= MAX_FILES) {
        truncated = true;
        truncationReason = `文件数超过结构化展示上限 ${MAX_FILES}`;
        break;
      }
      const paths = parseDiffHeaderPaths(line.slice("diff --git ".length));
      if (!paths) return fail("无法解析 diff --git 路径");
      current = {
        oldPath: stripSidePrefix(paths[0], "a/"), newPath: stripSidePrefix(paths[1], "b/"),
        change: "modified", oldMode: null, newMode: null, similarity: null, binary: false,
        hunks: [], additions: 0, deletions: 0, currentHunk: null, binaryPayload: false, movementFrom: false, movementTo: false,
      };
      continue;
    }
    if (!current) {
      if (line === "") continue;
      return fail("缺少 diff --git 文件头");
    }
    if (current.binaryPayload) continue;

    if (line.startsWith("@@")) {
      const invalid = finishHunk();
      if (invalid) return invalid;
      const match = HUNK_HEADER.exec(line);
      if (!match) return fail("无法解析 unified hunk 头部");
      if (hunkCount >= MAX_HUNKS) {
        truncated = true;
        truncationReason = `hunk 数超过结构化展示上限 ${MAX_HUNKS}`;
        break;
      }
      const hunk: MutableHunk = {
        header: line, section: match[5] || null,
        oldStart: Number(match[1]), oldCount: match[2] === undefined ? 1 : Number(match[2]),
        newStart: Number(match[3]), newCount: match[4] === undefined ? 1 : Number(match[4]),
        lines: [], seenOld: 0, seenNew: 0,
      };
      if (![hunk.oldStart, hunk.oldCount, hunk.newStart, hunk.newCount,
        hunk.oldStart + hunk.oldCount, hunk.newStart + hunk.newCount].every(Number.isSafeInteger)) {
        return fail("hunk 行号超出安全整数范围");
      }
      if ((hunk.oldCount > 0 && hunk.oldStart < 1) || (hunk.newCount > 0 && hunk.newStart < 1)) return fail("非空 hunk 的行号必须从 1 开始");
      current.hunks.push(hunk);
      current.currentHunk = hunk;
      hunkCount += 1;
      continue;
    }

    const hunk = current.currentHunk;
    if (hunk) {
      if (lineCount >= MAX_LINES) {
        truncated = true;
        truncationReason = `变更行超过结构化展示上限 ${MAX_LINES}`;
        break;
      }
      let parsed: DiffLine;
      if (line.startsWith(" ")) {
        parsed = { kind: "context", oldLine: hunk.oldStart + hunk.seenOld, newLine: hunk.newStart + hunk.seenNew, text: line.slice(1) };
        hunk.seenOld += 1; hunk.seenNew += 1;
      } else if (line.startsWith("+")) {
        parsed = { kind: "addition", oldLine: null, newLine: hunk.newStart + hunk.seenNew, text: line.slice(1) };
        hunk.seenNew += 1; current.additions += 1; additions += 1;
      } else if (line.startsWith("-")) {
        parsed = { kind: "deletion", oldLine: hunk.oldStart + hunk.seenOld, newLine: null, text: line.slice(1) };
        hunk.seenOld += 1; current.deletions += 1; deletions += 1;
      } else if (line === "\\ No newline at end of file") {
        parsed = { kind: "no_newline", oldLine: null, newLine: null, text: line };
      } else {
        return fail("hunk 中出现无法识别的行");
      }
      hunk.lines.push(parsed);
      lineCount += 1;
      continue;
    }

    if (line === "GIT binary patch" || line.startsWith("Binary files ")) {
      current.binary = true;
      if (current.change === "modified") current.change = "binary";
      current.binaryPayload = line === "GIT binary patch";
    } else if (line.startsWith("new file mode ")) {
      if (!/^[0-7]{6}$/.test(line.slice(14))) return fail("文件模式无效");
      current.change = "added"; current.newMode = line.slice(14);
    } else if (line.startsWith("deleted file mode ")) {
      if (!/^[0-7]{6}$/.test(line.slice(18))) return fail("文件模式无效");
      current.change = "deleted"; current.oldMode = line.slice(18);
    } else if (line.startsWith("old mode ")) {
      if (!/^[0-7]{6}$/.test(line.slice(9))) return fail("文件模式无效");
      current.oldMode = line.slice(9);
    } else if (line.startsWith("new mode ")) {
      if (!/^[0-7]{6}$/.test(line.slice(9))) return fail("文件模式无效");
      current.newMode = line.slice(9);
    } else if (line.startsWith("similarity index ")) {
      current.similarity = line.slice(17);
    } else if (line.startsWith("dissimilarity index ")) {
      // Retain the fact without incorrectly labelling dissimilarity as similarity.
      current.similarity = `差异度 ${line.slice(20)}`;
    } else if (line.startsWith("rename from ")) {
      current.oldPath = parseSinglePath(line.slice(12)); current.change = "renamed"; current.movementFrom = true;
    } else if (line.startsWith("rename to ")) {
      current.newPath = parseSinglePath(line.slice(10)); current.change = "renamed"; current.movementTo = true;
    } else if (line.startsWith("copy from ")) {
      current.oldPath = parseSinglePath(line.slice(10)); current.change = "copied"; current.movementFrom = true;
    } else if (line.startsWith("copy to ")) {
      current.newPath = parseSinglePath(line.slice(8)); current.change = "copied"; current.movementTo = true;
    } else if (line.startsWith("--- ")) {
      const path = parseSinglePath(line.slice(4));
      if (path === "/dev/null") current.change = "added"; else current.oldPath = stripSidePrefix(path, "a/");
    } else if (line.startsWith("+++ ")) {
      const path = parseSinglePath(line.slice(4));
      if (path === "/dev/null") current.change = "deleted"; else current.newPath = stripSidePrefix(path, "b/");
    } else if (line.startsWith("index ")) {
      const indexMetadata = /^index [0-9a-f]+\.\.[0-9a-f]+(?: ([0-7]{6}))?$/.exec(line);
      if (!indexMetadata) return fail("index 元数据无效");
      if (indexMetadata[1]) { current.oldMode ??= indexMetadata[1]; current.newMode ??= indexMetadata[1]; }
    } else if (line === "") {
      // Blank separators do not add facts.
    } else {
      return fail("文件元数据行无法识别");
    }
  }

  if (!truncated) {
    const invalid = finishFile();
    if (invalid) return invalid;
  } else if (current) {
    current.currentHunk = null;
    const { currentHunk: _hunk, binaryPayload: _payload, movementFrom: _from, movementTo: _to, ...file } = current;
    files.push(file);
  }
  return { kind: "structured", diff: { files, additions, deletions, truncated, truncationReason } };
}

function emptyDiff(): StructuredDiff {
  return { files: [], additions: 0, deletions: 0, truncated: false, truncationReason: null };
}

function parseDiffHeaderPaths(value: string): [string, string] | null {
  if (value.length > 16_384) return null;
  const tokens = value.match(/"(?:\\.|[^"\\])*"|\S+/g);
  if (tokens?.length === 2) return [decodeGitToken(tokens[0]!), decodeGitToken(tokens[1]!)];
  // Git leaves ordinary spaces unquoted. A quoted first path has an unambiguous end;
  // otherwise use the b/ boundary and only resolve multiple boundaries for equal paths.
  if (value.startsWith('"')) {
    const first = /^"(?:\\.|[^"\\])*" /.exec(value);
    if (!first) return null;
    return [decodeGitToken(first[0].slice(0, -1)), decodeGitToken(value.slice(first[0].length))];
  }
  const quotedSecond = value.indexOf(' "b/');
  if (quotedSecond !== -1) return [value.slice(0, quotedSecond), decodeGitToken(value.slice(quotedSecond + 1))];
  const candidates: [string, string][] = [];
  for (let at = value.indexOf(" b/"); at !== -1; at = value.indexOf(" b/", at + 1)) {
    candidates.push([value.slice(0, at), value.slice(at + 1)]);
  }
  if (candidates.length === 1) return candidates[0]!;
  return candidates.find(([oldPath, newPath]) => stripSidePrefix(oldPath, "a/") === stripSidePrefix(newPath, "b/")) ?? null;
}

function parseSinglePath(value: string): string {
  // Git's ---/+++ headers may terminate an unquoted path with a tab, not a timestamp.
  return decodeGitToken(value.endsWith("\t") ? value.slice(0, -1) : value);
}

function stripSidePrefix(path: string, prefix: "a/" | "b/"): string {
  return path.startsWith(prefix) ? path.slice(2) : path;
}

function decodeGitToken(token: string): string {
  if (!(token.startsWith('"') && token.endsWith('"'))) return token;
  const bytes: number[] = [];
  const encoder = new TextEncoder();
  const appendText = (value: string) => bytes.push(...encoder.encode(value));
  for (let index = 1; index < token.length - 1; index += 1) {
    const value = token[index]!;
    if (value !== "\\") {
      const character = String.fromCodePoint(token.codePointAt(index)!);
      appendText(character); index += character.length - 1; continue;
    }
    const escaped = token[++index];
    if (escaped === undefined) return token;
    if (/[0-7]/.test(escaped)) {
      let octal = escaped;
      for (let count = 0; count < 2 && /[0-7]/.test(token[index + 1] ?? ""); count += 1) octal += token[++index];
      const byte = Number.parseInt(octal, 8);
      if (byte > 255) return token;
      bytes.push(byte);
    } else {
      const controls: Record<string, string> = { a: "\u0007", b: "\b", t: "\t", n: "\n", v: "\u000b", f: "\f", r: "\r", "\\": "\\", '"': '"' };
      if (!(escaped in controls)) return token;
      appendText(controls[escaped]!);
    }
  }
  try { return new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(bytes)); }
  catch { return token; }
}
