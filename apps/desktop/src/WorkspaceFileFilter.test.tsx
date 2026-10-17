// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { WorkspaceFileFilter } from "./WorkspaceFileFilter";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
it("debounces filtering and opens matches with their root and path", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.useFakeTimers();
  invoke.mockResolvedValue({ files: [{ root: 1, path: "src/Readme.md" }], partial: false });
  const container = document.createElement("div"); const root = createRoot(container); const onSelect = vi.fn();
  const render = (query: string, hideNoise = false) => act(async () => root.render(<WorkspaceFileFilter taskId="task" query={query} roots={["/one", "/two"]} selected={null} onSelect={onSelect} visible hideNoise={hideNoise} />));
  try {
    await render("read");
    await act(async () => vi.advanceTimersByTime(200));
    await render("readme");
    await act(async () => vi.advanceTimersByTime(299));
    expect(invoke).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTime(1));
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("two");
    expect(container.textContent).toContain("src");
    await act(async () => (container.querySelector('button') as HTMLElement).click());
    expect(onSelect).toHaveBeenCalledWith({ root: 1, path: "src/Readme.md" });
    await render("readme", true);
    await act(async () => vi.advanceTimersByTime(300));
    expect(invoke).toHaveBeenLastCalledWith("core_workspace_find_files", { taskId: "task", query: "readme", hideNoise: true });
  } finally { await act(async () => root.unmount()); vi.useRealTimers(); vi.unstubAllGlobals(); }
});

it("keeps descendants hidden after collapse all until each ancestor is expanded", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.useFakeTimers();
  invoke.mockResolvedValue({ files: [{ root: 0, path: "pages/nested/index.ts" }, { root: 0, path: "src/index.ts" }, { root: 0, path: "index.html" }], partial: false });
  const container = document.createElement("div"), root = createRoot(container);
  const render = (collapseVersion: number) => act(async () => root.render(<WorkspaceFileFilter taskId="task" query="index" roots={["/repo"]} selected={null} onSelect={() => {}} visible collapseVersion={collapseVersion} />));
  const folder = (name: string) => [...container.querySelectorAll('details')].find(node => node.querySelector('summary')?.textContent === name)!;
  const children = (name: string) => folder(name).querySelector<HTMLDivElement>(':scope > .review-tree-children')!;
  const toggle = (name: string) => act(async () => folder(name).querySelector('summary')!.click());
  try {
    await render(0); await act(async () => vi.advanceTimersByTime(300));
    await render(1);
    for (const name of ['repo', 'pages', 'nested', 'src']) expect(children(name).hidden).toBe(true);
    await toggle('repo');
    expect(children('repo').hidden).toBe(false);
    expect(children('pages').hidden).toBe(true); expect(children('src').hidden).toBe(true);
    await toggle('pages');
    expect(children('pages').hidden).toBe(false); expect(children('nested').hidden).toBe(true);
    await toggle('nested'); expect(children('nested').hidden).toBe(false);
    await render(2); await toggle('repo');
    expect(children('pages').hidden).toBe(true); expect(children('nested').hidden).toBe(true);
  } finally { await act(async () => root.unmount()); vi.useRealTimers(); vi.unstubAllGlobals(); }
});

it("offers working empty-search actions and explains hidden files", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.useFakeTimers();
  invoke.mockResolvedValue({ files: [], partial: false });
  const container = document.createElement("div"), root = createRoot(container);
  const onClear = vi.fn(), onShowAll = vi.fn();
  try {
    await act(async () => root.render(<WorkspaceFileFilter taskId="task" query="missing" roots={["/repo"]} selected={null} onSelect={() => {}} visible hideNoise onClear={onClear} onShowAll={onShowAll} />));
    expect(container.querySelector(".review-empty-state")).toBeNull();
    await act(async () => vi.advanceTimersByTime(300));
    expect(container.textContent).toContain("当前已隐藏");
    await act(async () => [...container.querySelectorAll("button")].find(button => button.textContent === "清空筛选")!.click());
    await act(async () => [...container.querySelectorAll("button")].find(button => button.textContent === "显示全部文件")!.click());
    expect(onClear).toHaveBeenCalledTimes(1); expect(onShowAll).toHaveBeenCalledTimes(1);
  } finally { await act(async () => root.unmount()); vi.useRealTimers(); vi.unstubAllGlobals(); }
});

it.each(["success", "failure"])("replaces the search animation after a pending read settles: %s", async outcome => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.useFakeTimers();
  let resolve!: (value: unknown) => void, reject!: (error: Error) => void;
  invoke.mockImplementation(() => new Promise((yes, no) => { resolve = yes; reject = no; }));
  const container = document.createElement("div"), root = createRoot(container);
  try {
    await act(async () => root.render(<WorkspaceFileFilter taskId="task" query="missing" roots={["/repo"]} selected={null} onSelect={() => {}} visible />));
    expect(container.querySelector('[role="status"] .loading-skeleton')).not.toBeNull();
    await act(async () => vi.advanceTimersByTime(300));
    expect(container.querySelector('.review-empty-state')).toBeNull();
    await act(async () => outcome === "success" ? resolve({ files: [], partial: false }) : reject(new Error("搜索失败")));
    expect(container.querySelector('.loading-status')).toBeNull();
    expect(container.textContent).toContain(outcome === "success" ? "没有匹配文件" : "搜索失败");
  } finally { await act(async () => root.unmount()); vi.useRealTimers(); vi.unstubAllGlobals(); }
});
