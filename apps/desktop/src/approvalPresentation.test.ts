import { expect, it } from "vitest";
import { approvalPresentation } from "./approvalPresentation";
it.each([
  ['{"command":"curl -s -I https://httpbin.org/get | head -5","timeout":30000}', "curl -s -I https://httpbin.org/get | head -5"],
  ['"echo hello"', "echo hello"],
  ['{"CommandLine":"echo hello","SafeToAutoRun":false}', "echo hello"],
  ['{"rawInput":{"command":"echo hello"},"toolCallId":"internal"}', "echo hello"],
  ['{"command":["bash","-lc","echo hello"]}', "bash -lc 'echo hello'"],
])("extracts native command %s", (summary, command) => {
  const view = approvalPresentation(summary, "run_command");
  expect(view.sections).toEqual([{ label: "命令", text: command, expandable: false }]);
  expect(view.category).toBe("终端");
});
it("shows file paths, moves and patches without metadata", () => {
  const view = approvalPresentation(JSON.stringify({ files: { "a.txt": { kind: { type: "update", movePath: "b.txt" }, diff: "-before\n+after" } }, itemId: "private-id", scope: "reported_files" }), "fileChange");
  expect(view.sections.map(s => s.text)).toEqual(["a.txt", "b.txt", "-before\n+after"]);
  expect(view.sections[2].expandable).toBe(true);
  expect(JSON.stringify(view)).not.toContain("private-id");
});
it("handles reads, writes, URLs and unknown inputs without raw JSON", () => {
  expect(approvalPresentation('{"file_path":"a.txt"}', "Read").category).toBe("读取文件");
  expect(approvalPresentation('{"TargetFile":"a.txt","ReplacementChunks":[{"TargetContent":"old","ReplacementContent":"new"}]}', "replace_file_content").sections.map(s => s.text)).toEqual(["a.txt", "old", "new"]);
  expect(approvalPresentation('{"url":"https://example.com","timeout":30}', "WebFetch").sections[0].text).toBe("https://example.com");
  for (const summary of ['{"unknown":{"debug":"internal"}}', '{"broken":', '']) {
    const view = approvalPresentation(summary, "tool_12");
    expect(view.sections).toEqual([{ label: "请求的工具", text: "tool_12", expandable: false }]); expect(view.warning).toBeTruthy();
  }
});

it("shows long structured edits verbatim without masking values", () => {
  const old = "const value = 1;\n".repeat(100) + 'api_key = "sk-example12345678"';
  const view = approvalPresentation(JSON.stringify({ input: { file_path: "src/model.ts", old_string: old, new_string: "deepseek-flash" }, detailsTruncated: false }), "Edit");
  expect(view.sections.map(s => s.text)).toEqual(["src/model.ts", old, "deepseek-flash"]);
  expect(view.warning).toBeUndefined();
});
it("rejects incomplete approval JSON without reconstructing partial fields", () => {
  const view = approvalPresentation('{"file_path":"src/model.ts","old_string":"[REDACTED_TOKEN]","new_string":"unfinished', "Edit");
  expect(view.sections.map(s => s.text)).toEqual(["Edit"]);
  expect(view.warning).toContain("格式不完整");
  expect(JSON.stringify(view)).not.toContain("unfinished");
});
it("reports bounded previews without claiming full details", () => {
  const view = approvalPresentation(JSON.stringify({ input: { file_path: "a.ts", new_string: "partial" }, detailsTruncated: true }), "Edit");
  expect(view.warning).toContain("仅展示部分内容");
  expect(view.sections[0].text).toBe("a.ts");
});
