// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ReviewFileTree, buildReviewTree } from "./ReviewFileTree";
import type { RootChanges } from "./reviewClient";
let root: Root, container: HTMLDivElement;
const onSelect = vi.fn();
const result: RootChanges = { root: { id: "r", path: "/repo", git: true, scopes: [""] }, mode: "unstaged", base: "index", target: "working", error: null, statistics: { "src/a.ts": { additions: 12, deletions: 3, binary: false, error: null } }, untracked: ["new/a.ts"], files: ["src/a.ts", "src/nested/b.ts", "new/a.ts", "__proto__/a.ts"].map(path => ({ path, kind: "modified", before: null, after: null })) };
const render = (filter = "") => act(async () => root.render(<ReviewFileTree result={result} filter={filter} busy={false} selectedPath="src/a.ts" onSelect={onSelect} />));
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); onSelect.mockReset(); container = document.createElement("div"); document.body.append(container); root = createRoot(container); });
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
it("builds real directories safely including prototype-like names", () => {
  const tree = buildReviewTree(result.files);
  expect(tree.directories.get("src")!.directories.get("nested")!.files[0].path).toBe("src/nested/b.ts");
  expect(tree.directories.get("__proto__")!.files).toHaveLength(1);
});
it("explains excluded repository boundaries even when there are no changes", async () => {
  await act(async () => root.render(<ReviewFileTree result={{ ...result, files: [], excludedRoots: ["packages/child", "old-repository"] }} filter="unmatched" busy={false} onSelect={onSelect} />));
  expect(container.textContent).toContain("已排除 2 个子仓库目录");
  expect(container.textContent).toContain("不计入父目录差异");
  expect(container.textContent).toContain("old-repository");
  expect(container.textContent).toContain("没有变更");
  expect(container.querySelectorAll(".review-file-entry")).toHaveLength(0);
});
it("separates untracked paths and selects full paths rather than duplicate names", async () => {
  await render();
  expect(container.querySelector('[aria-label="new/a.ts"] .review-status')?.textContent).toBe("U");
  expect(container.querySelector('[aria-label="src/a.ts"] .review-status')?.textContent).toBe("M");
  await act(async () => (container.querySelector('[aria-label="src/a.ts"]') as HTMLElement).click());
  expect(onSelect).toHaveBeenCalledWith("src/a.ts");
  expect(container.querySelector('[aria-label="src/a.ts"]')?.textContent).toContain("+12 −3");
  expect(container.querySelector('[aria-current="true"]')?.getAttribute("data-tooltip")).toBe("src/a.ts");
});
it("filter reveals matches without forgetting directory folds", async () => {
  await render();
  await act(async () => (container.querySelector('[data-tooltip="src"]') as HTMLElement).click());
  expect(container.querySelector('[aria-label="src/a.ts"]')).toBeNull();
  await render("src/a");
  expect(container.querySelector('[aria-label="src/a.ts"]')).not.toBeNull();
  await render();
  expect(container.querySelector('[aria-label="src/a.ts"]')).toBeNull();
  await render("no such path");
  expect(container.textContent).toContain("没有匹配文件");
});
it("keeps paired renames in changes and finds them by the old path", async () => {
  const renamed: RootChanges = { ...result, untracked: ["new/name.ts", "loose.txt"], files: [
    { path: "new/name.ts", previousPath: "old/name.ts", kind: "renamed", before: null, after: null },
    { path: "loose.txt", kind: "added", before: null, after: null },
  ] };
  await act(async () => root.render(<ReviewFileTree result={renamed} filter="" busy={false} onSelect={onSelect} />));
  expect(container.querySelector('[aria-label="new/name.ts"] .review-status')?.textContent).toBe("R");
  expect(container.querySelector('[aria-label="loose.txt"] .review-status')?.textContent).toBe("U");
  await act(async () => root.render(<ReviewFileTree result={renamed} filter="old/name" busy={false} onSelect={onSelect} />));
  const entry = container.querySelector('[aria-label="new/name.ts"]') as HTMLButtonElement;
  expect(entry.dataset.tooltip).toContain("old/name.ts → new/name.ts");
  await act(async () => entry.click());
  expect(onSelect).toHaveBeenCalledWith("new/name.ts");
});

it("shows directory names and aggregate counts without absolute paths or revision hashes", async () => {
  await render();
  const heading = container.querySelector(".review-root-heading")!;
  expect(heading.textContent).toContain("repo");
  expect(heading.textContent).toContain("+12 −3");
  expect(container.textContent).not.toContain("/repo");
  expect(container.textContent).not.toContain("index");
  await act(async () => (heading as HTMLElement).click());
  expect(container.querySelector(".review-file-entry")).toBeNull();
  await act(async () => (heading as HTMLElement).click());
  expect(container.querySelector(".review-file-entry")).not.toBeNull();
});
