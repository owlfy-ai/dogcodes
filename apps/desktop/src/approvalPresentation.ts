import { translate as t } from "./i18n";
type RecordValue = Record<string, unknown>;
export interface ApprovalSection { label: string; text: string; expandable?: boolean }
export interface ApprovalPresentation {
  kind: "terminal" | "readFile" | "editFile" | "network" | "search" | "confirm";
  category: string;
  question: string;
  sections: ApprovalSection[];
  warning?: string;
}
const object = (value: unknown): RecordValue | undefined => value !== null && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : undefined;
const text = (value: unknown): string => typeof value === "string" ? value : "";
const first = (value: RecordValue, keys: string[]) => keys.map(key => text(value[key])).find(Boolean) ?? "";
const quoteArgument = (value: string) => /^[a-zA-Z0-9_./:=@%-]+$/.test(value) ? value : `'${value.replaceAll("'", "'\\''")}'`;
const commandText = (value: unknown): string => typeof value === "string" ? value : Array.isArray(value) && value.every(v => typeof v === "string") ? value.map(quoteArgument).join(" ") : "";

/** Convert native approval inputs to reviewable content; protocol envelopes never reach the UI. */
export function approvalPresentation(summary: string, toolName = ""): ApprovalPresentation {
  let parsed: unknown = summary;
  let malformedEnvelope = false;
  try { parsed = JSON.parse(summary); } catch {
    // Do not expose truncated protocol objects; shell expressions remain plain text.
    malformedEnvelope = /^\s*(?:\{\s*"|\[\s*[{"])/.test(summary);
  }
  const truncated = object(parsed)?.detailsTruncated === true;
  let input = object(parsed);
  for (let depth = 0; input && depth < 4; depth++) {
    const nested = object(input.rawInput) ?? object(input.raw_input) ?? object(input.input) ?? object(input.arguments) ?? object(input.args);
    if (!nested) break;
    input = { ...input, ...nested, rawInput: undefined, raw_input: undefined, input: undefined, arguments: undefined, args: undefined };
  }
  const value = input ?? {};
  const command = commandText(value.command) || commandText(value.cmd) || commandText(value.command_line) || commandText(value.commandLine) || commandText(value.argv) || commandText(value.CommandLine);
  const path = first(value, ["file_path", "filePath", "path", "target_file", "targetFile", "TargetFile", "AbsolutePath"]);
  const url = first(value, ["url", "uri", "website"]);
  const fileLike = !!path || !!value.files || /read|edit|write|patch|filechange|修改|写入|文件/i.test(toolName);
  const terminal = !!command || /command|shell|bash|terminal|终端|命令/i.test(toolName) || (!input && !fileLike && !url);
  const reading = /read|读取/i.test(toolName);
  const kind = terminal ? "terminal" : fileLike ? (reading ? "readFile" : "editFile") : url ? "network" : /search|grep|glob|搜索/i.test(toolName) ? "search" : "confirm";
  const category = t(`approval.category.${kind}`);
  const question = t(terminal ? "approval.question.terminal" : fileLike ? (reading ? "approval.question.readFile" : "approval.question.editFile") : url ? "approval.question.network" : "approval.question.other");
  const sections: ApprovalSection[] = [];
  const add = (label: string, content: string, expandable = false) => { if (content.trim()) sections.push({ label, text: content, expandable }); };
  if (command) add(t("approval.section.command"), command);
  add(t("approval.section.details"), text(value.detailText), true);
  if (path) add(t("approval.section.file"), path);
  if (url) add(t("approval.section.url"), url);
  add(t("approval.section.grantRoot"), text(value.grantRoot));
  const fileSection = (file: unknown, fallbackPath = "") => {
    if (typeof file === "string") { add(t("approval.section.file"), file); return; }
    const entry = object(file);
    if (!entry) return;
    const kind = typeof entry.kind === "string" ? entry.kind : text(object(entry.kind)?.type);
    const label = t(kind === "delete" ? "approval.section.deleteFile" : kind === "add" ? "approval.section.newFile" : "approval.section.file");
    add(label, first(entry, ["path", "file_path", "filePath"]) || fallbackPath);
    const move = first(object(entry.kind) ?? entry, ["movePath", "move_path"]);
    add(t("approval.section.moveTo"), move);
    add(t("approval.section.changes"), first(entry, ["diff", "patch", "content"]), true);
  };
  if (Array.isArray(value.files)) value.files.forEach(file => fileSection(file));
  else if (object(value.files)) Object.entries(object(value.files)!).forEach(([name, file]) => fileSection(file, name));
  if (Array.isArray(value.ReplacementChunks)) for (const chunk of value.ReplacementChunks) {
    const entry = object(chunk);
    if (entry) { add(t("approval.section.before"), text(entry.TargetContent), true); add(t("approval.section.after"), text(entry.ReplacementContent), true); }
  }
  add(t("approval.section.changes"), first(value, ["diff", "patch", "unified_diff"]), true);
  add(t("approval.section.before"), first(value, ["old_string", "old_str", "oldText"]), true);
  add(t("approval.section.after"), first(value, ["new_string", "new_str", "newText"]), true);
  if (fileLike) add(t("approval.section.fileContent"), first(value, ["content", "CodeContent"]), true);
  add(t("approval.section.search"), first(value, ["query", "pattern", "glob"]));
  if (!input && typeof parsed === "string" && !malformedEnvelope) add(t(terminal ? "approval.section.command" : "approval.section.details"), parsed);
  const reason = first(value, ["justification", "reason", "description"]);
  const warnings: string[] = [];
  if (malformedEnvelope) warnings.push(t("approval.warning.malformed"));
  else if (truncated) warnings.push(t("approval.warning.truncated"));
  else if (value.scope === "unknown") warnings.push(t("approval.warning.unknownScope"));
  else if (sections.length === 0) warnings.push(t("approval.warning.empty"));
  if (sections.length === 0 && toolName) add(t("approval.section.tool"), toolName);
  const warning = warnings.length ? warnings.join(" ") : undefined;
  return { kind, category, question: reason || question, sections, warning };
}
