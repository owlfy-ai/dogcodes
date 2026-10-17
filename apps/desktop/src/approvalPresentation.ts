type RecordValue = Record<string, unknown>;
export interface ApprovalSection { label: string; text: string; expandable?: boolean }
export interface ApprovalPresentation {
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
  const category = terminal ? "终端" : fileLike ? (reading ? "读取文件" : "修改文件") : url ? "网络访问" : /search|grep|glob|搜索/i.test(toolName) ? "搜索" : "操作确认";
  const question = terminal ? "是否允许运行以下命令？" : fileLike ? (reading ? "是否允许读取以下文件？" : "是否允许修改以下文件？") : url ? "是否允许访问以下地址？" : "是否允许执行以下操作？";
  const sections: ApprovalSection[] = [];
  const add = (label: string, content: string, expandable = false) => { if (content.trim()) sections.push({ label, text: content, expandable }); };
  if (command) add("命令", command);
  add("操作内容", text(value.detailText), true);
  if (path) add("文件", path);
  if (url) add("地址", url);
  add("授权目录", text(value.grantRoot));
  const fileSection = (file: unknown, fallbackPath = "") => {
    if (typeof file === "string") { add("文件", file); return; }
    const entry = object(file);
    if (!entry) return;
    const kind = typeof entry.kind === "string" ? entry.kind : text(object(entry.kind)?.type);
    const label = kind === "delete" ? "删除文件" : kind === "add" ? "新建文件" : "文件";
    add(label, first(entry, ["path", "file_path", "filePath"]) || fallbackPath);
    const move = first(object(entry.kind) ?? entry, ["movePath", "move_path"]);
    add("移动到", move);
    add("修改内容", first(entry, ["diff", "patch", "content"]), true);
  };
  if (Array.isArray(value.files)) value.files.forEach(file => fileSection(file));
  else if (object(value.files)) Object.entries(object(value.files)!).forEach(([name, file]) => fileSection(file, name));
  if (Array.isArray(value.ReplacementChunks)) for (const chunk of value.ReplacementChunks) {
    const entry = object(chunk);
    if (entry) { add("替换前", text(entry.TargetContent), true); add("替换后", text(entry.ReplacementContent), true); }
  }
  add("修改内容", first(value, ["diff", "patch", "unified_diff"]), true);
  add("替换前", first(value, ["old_string", "old_str", "oldText"]), true);
  add("替换后", first(value, ["new_string", "new_str", "newText"]), true);
  if (fileLike) add("文件内容", first(value, ["content", "CodeContent"]), true);
  add("搜索内容", first(value, ["query", "pattern", "glob"]));
  if (!input && typeof parsed === "string" && !malformedEnvelope) add(terminal ? "命令" : "操作内容", parsed);
  const reason = first(value, ["justification", "reason", "description"]);
  const warnings: string[] = [];
  if (malformedEnvelope) warnings.push("审批详情格式不完整，请让 Agent 重新发起审批。");
  else if (truncated) warnings.push("操作详情过长，下方仅展示部分内容。请让 Agent 拆分操作后重新发起审批，以查看完整范围。");
  else if (value.scope === "unknown") warnings.push("Agent 未提供文件修改范围，请让 Agent 补充文件路径和修改内容。");
  else if (sections.length === 0) warnings.push("未能从审批请求中读取文件路径、命令或修改内容，请让 Agent 补充详情后重新发起审批。");
  if (sections.length === 0 && toolName) add("请求的工具", toolName);
  const warning = warnings.length ? warnings.join(" ") : undefined;
  return { category, question: reason || question, sections, warning };
}
