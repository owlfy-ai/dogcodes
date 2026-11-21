import { expect, it } from "vitest";
import type { TimelineItem } from "@dogcodes/protocol";
import { buildTurnArtifact, confirmedTurnPaths } from "./turnEditPaths";
import { editPatch, frozenDiff } from "./conversationDiff";
import type { TurnArtifact } from "./turnArtifactCache";

function tool(turnId: string, fields: Record<string, unknown>): TimelineItem {
  return { turnId, kind: "activity", providerId: "zcode",
    text: JSON.stringify({ type: "tool", status: "completed", ...fields }) } as TimelineItem;
}
it("attributes successful edits by turn, including ZCode results without tool inputs", () => {
  const turns = confirmedTurnPaths([
    tool("a", { name: "Edit", output: { display: { kind: "file_diff", filePath: "/project/both.ts" } } }),
    tool("a", { name: "Write", output: { success: true, content: "File created successfully at: /project/a.ts" } }),
    tool("b", { name: "edit", changes: [{ path: "/project/both.ts" }, { path: "/project/b.ts" }] }),
    tool("a", { name: "Edit", status: "failed", input: { file_path: "b.ts" } }),
    tool("a", { name: "Write", status: "running", input: { path: "b.ts" } }),
    tool("a", { name: "Read", input: { path: "b.ts" } }),
    tool("a", { name: "Bash", input: { path: "b.ts" } }),
  ]);
  expect(turns.get("a")).toEqual(new Set(["/project/both.ts", "/project/a.ts"]));
  expect(turns.get("b")).toEqual(new Set(["/project/both.ts", "/project/b.ts"]));
});

it("keeps the whole shared-file diff, drops unrelated paths and recounts the visible patch", () => {
  const data = { workspacePaths: ["/work/sub"], files: ["a", "b"].map(c => ({ path: "sub/" + c, workspaceRelativePath: c })),
    patch: editPatch("sub/a", "old\n", "mine\ntheirs\n") + editPatch("sub/b", "", "other only\n") };
  const parsed = frozenDiff(data);
  if (parsed.kind !== "structured") throw Error("invalid fixture");
  const original: TurnArtifact = { data, diff: parsed.diff, expanded: false };
  const view = buildTurnArtifact(original.data, new Set(["./a"]));
  expect(view.data.files.map(file => file.path)).toEqual(["sub/a"]);
  expect(view.diff?.files).toEqual([original.diff!.files[0]]);
  expect(view.diff).toMatchObject({ additions: 2, deletions: 1 });
  expect(view.data.patch).toBeUndefined();
  expect(original.data.files).toHaveLength(2);
  expect(buildTurnArtifact(original.data, new Set()).data.files).toEqual([]);
});

it("matches the exact workspace for duplicate relative paths across roots", () => {
  const data = { workspacePaths: ["/one", "/two"], files: ["/one", "/two"].map(workspacePath =>
    ({ path: workspacePath + "/src/a", workspacePath, relativePath: "src/a" })) };
  const capture = { data, expanded: false, diff: null };
  expect(buildTurnArtifact(capture.data, new Set(["src/a"])).data.files).toEqual([data.files[0]]);
  expect(buildTurnArtifact(capture.data, new Set(["/two/src/a"])).data.files).toEqual([data.files[1]]);
});
