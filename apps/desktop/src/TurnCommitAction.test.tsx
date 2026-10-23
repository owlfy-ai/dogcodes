// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { TurnCommitAction } from "./TurnCommitAction";
import { turnCommitTargets, turnAbsolutePaths } from "./turnCommit";
import type { RootChanges } from "./reviewClient";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./availableAgentExecutors", () => ({ availableAgentExecutors: async () => [{ providerId: "codex", installationId: "i-codex" }, { providerId: "claude-code", installationId: "i-claude" }] }));

const root = (id: string, path: string, files: string[], untracked: string[] = []) => ({
  root: { id, path, git: true, scopes: [] }, mode: "uncommitted", base: "", target: "", untracked, error: null, statistics: {},
  files: files.map(p => ({ path: p, kind: "modified", before: null, after: null })),
}) as unknown as RootChanges;

it("resolves v1 and v2 turn files to the roots that still have them uncommitted", () => {
  expect(turnAbsolutePaths({ files: [{ path: "src/a.ts" }], workspacePaths: ["/repo/"] })).toEqual(["/repo/src/a.ts"]);
  const v2 = { files: [
    { path: "/w/one/a.md", workspacePath: "/w/one", relativePath: "a.md" },
    { path: "/w/two/b.md", workspacePath: "/w/two", relativePath: "b.md" },
    { path: "/w/two/gone.md", workspacePath: "/w/two", relativePath: "gone.md" },
  ], workspacePaths: ["/w/one", "/w/two"] };
  expect(turnCommitTargets(v2, [root("r1", "/w/one", ["a.md", "other.md"]), root("r2", "/w/two", [], ["b.md"]), root("r3", "/w/three", ["x"])]))
    .toEqual([{ rootId: "r1", rootPath: "/w/one", paths: ["a.md"] }, { rootId: "r2", rootPath: "/w/two", paths: ["b.md"] }]);
});

let host: HTMLDivElement, view: Root;
const button = (text: string) => [...document.querySelectorAll("button")].find(b => b.textContent === text)!;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  localStorage.clear();
  host = document.createElement("div"); document.body.append(host); view = createRoot(host);
});
afterEach(async () => { await act(async () => view.unmount()); host.remove(); localStorage.clear(); vi.unstubAllGlobals(); invoke.mockReset(); });

function backend() {
  const index = new Set<string>(["already.txt"]);
  invoke.mockImplementation(async (name: string, args: Record<string, unknown>) => {
    if (name === "core_review_query") {
      const query = args.query as { mode: string };
      return query.mode === "staged"
        ? { id: `staged-${index.size}`, taskId: "tsk", roots: [root("r1", "/repo", [...index])] }
        : { id: "u1", taskId: "tsk", roots: [root("r1", "/repo", ["a.ts", "b.ts", "unrelated.ts", "already.txt"])] };
    }
    if (name === "core_review_index") {
      const paths = args.paths as string[];
      for (const p of paths) if (args.action === "stage") index.add(p); else index.delete(p);
      return null;
    }
    if (name === "core_review_commit_summary") return "feat: update a and b\n";
    if (name === "core_review_git_action") return true;
    return null;
  });
  return index;
}
const data = { files: [{ path: "a.ts", status: "M" }, { path: "b.ts", status: "M" }], workspacePaths: ["/repo"] } as never;

it("stages only this turn's files, generates the message from them and commits", async () => {
  const index = backend();
  await act(async () => view.render(<TurnCommitAction taskId="tsk" data={data} providerId="claude-code" />));
  await act(async () => button("提交").click());
  expect([...index].sort()).toEqual(["a.ts", "already.txt", "b.ts"]);
  expect(invoke).toHaveBeenCalledWith("core_review_commit_summary", { taskId: "tsk", rootId: "r1", executor: { providerId: "claude-code", installationId: "i-claude" }, language: "zh-CN" });
  expect((document.querySelector("textarea") as HTMLTextAreaElement).value).toBe("feat: update a and b");
  expect(document.querySelector(".turn-commit-dialog")?.textContent).toContain("其中 1 个是此前已暂存的其他更改");
  await act(async () => [...document.querySelectorAll("dialog button")].find(b => b.textContent === "提交")!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  const commit = invoke.mock.calls.find(([name]) => name === "core_review_git_action")![1];
  expect(commit).toMatchObject({ taskId: "tsk", rootId: "r1", action: "commit", message: "feat: update a and b", stageAll: false, snapshotId: "staged-3" });
  expect(button("已提交").disabled).toBe(true);
});

it("cancel restores the index entries it staged", async () => {
  const index = backend();
  await act(async () => view.render(<TurnCommitAction taskId="tsk" data={data} />));
  await act(async () => button("提交").click());
  await act(async () => button("取消").click());
  expect([...index]).toEqual(["already.txt"]);
  expect(invoke.mock.calls.some(([name]) => name === "core_review_git_action")).toBe(false);
});

it("explains when the turn has nothing left to commit", async () => {
  invoke.mockImplementation(async () => ({ id: "u", taskId: "tsk", roots: [root("r1", "/repo", ["other.ts"])] }));
  await act(async () => view.render(<TurnCommitAction taskId="tsk" data={data} />));
  await act(async () => button("提交").click());
  expect(document.querySelector(".turn-commit-dialog")?.textContent).toContain("已没有未提交的内容");
  expect(invoke.mock.calls.some(([name]) => name === "core_review_index")).toBe(false);
});
