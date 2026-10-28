// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ConversationReviewDocks, ReviewDock, ReviewPanel } from "./ReviewPanel";
import type { ReviewFile, ReviewSnapshot } from "./reviewClient";
import type { CoreClient, TaskId } from "@dogcodes/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import * as tauri from "@tauri-apps/api/core";
import type { ReviewWorkspace } from "./ReviewWorkspaceBrowser";

vi.mock("@tauri-apps/api/core", async importOriginal => ({
  ...await importOriginal<typeof import("@tauri-apps/api/core")>(),
  invoke: vi.fn(),
}));
vi.mock("./gitOperations", async importOriginal => ({ ...await importOriginal<typeof import("./gitOperations")>(), gitOperations: {
  run: (args: Record<string, unknown>) => tauri.invoke("core_review_git_action", args),
  advance: (taskId: string, operationId: string) => tauri.invoke("core_review_git_advance", { taskId, operationId }),
} }));
vi.mock("./ProjectRunPanel", () => ({ ProjectRunPanel: () => <div>运行命令</div> }));

let root: Root, container: HTMLDivElement;
const api = { batchFiles: vi.fn(), lease: vi.fn(), query: vi.fn(), file: vi.fn(), history: vi.fn(), branches: vi.fn(), index: vi.fn(), discard: vi.fn(), preview: vi.fn(), reveal: vi.fn(), ignore: vi.fn() };
const snapshot: ReviewSnapshot = { id: "snapshot", taskId: "task", roots: [{ root: { id: "repo", path: "/repo", git: true, scopes: [""] }, mode: "uncommitted", base: "HEAD", target: "working", untracked: ["b"], error: null, statistics: {},
  files: ["a", "b"].map(path => ({ path, kind: "added", before: null, after: null })) }] };
const file: ReviewFile = { binary: false, text: { before: [], after: ["new\n"], additions: 1, deletions: 0, blocks: [{ kind: "insert", oldStart: 0, oldEnd: 0, newStart: 0, newEnd: 1 }] } };
const render = (running = false) => act(async () => root.render(<ReviewPanel taskId="task" running={running} onClose={() => {}} api={api} />));
const click = async (element: Element) => act(async () => { (element as HTMLElement).click(); });
const select = async (label: string, value: string) => act(async () => {
  const element = container.querySelector(`[aria-label="${label}"]`) as HTMLSelectElement;
  element.value = value; element.dispatchEvent(new Event("change", { bubbles: true }));
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const loadingIndicator = () => container.querySelector('[aria-label="正在加载工作区差异"]');
const readyAnalysis = () => ({ bootstrap: vi.fn(async () => ({ installations: [] })), turnReviews: vi.fn(async () => []) }) as unknown as CoreClient;

it("includes the full loading surface in the first open render before effects run", () => {
  const html = renderToStaticMarkup(<ReviewDock open taskId="task" running={false} onClose={() => {}} api={api} />);
  expect(html).toContain('aria-label="正在加载工作区差异"');
  expect(html).toContain('aria-label="关闭 Review"');
  expect(html).toContain('inert=""');
});

it("reveals the initial panel only after diff, review history and agents have settled", async () => {
  vi.useFakeTimers();
  const query = deferred<ReviewSnapshot>();
  const history = deferred<[]>();
  const agents = deferred<{ installations: [] }>();
  const analysisClient = { bootstrap: () => agents.promise, turnReviews: () => history.promise } as unknown as CoreClient;
  api.query.mockReturnValue(query.promise);
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} analysisClient={analysisClient} />));
  expect(loadingIndicator()).not.toBeNull();
  await act(async () => history.resolve([]));
  expect(loadingIndicator()).not.toBeNull();
  await act(async () => query.resolve(snapshot));
  expect(container.querySelector('.review-changes-content')?.hasAttribute('inert')).toBe(true);
  await act(async () => agents.resolve({ installations: [] }));
  expect(loadingIndicator()).toBeNull();
  expect(container.querySelector('.review-changes-content')?.hasAttribute('inert')).toBe(false);
  expect(container.querySelector('button[aria-label="a"]')).not.toBeNull();
  const refresh = deferred<ReviewSnapshot>();
  api.query.mockReturnValue(refresh.promise);
  await click(container.querySelector('[aria-label="刷新"]')!);
  expect(loadingIndicator()).toBeNull();
  expect(container.querySelector('button[aria-label="a"]')).not.toBeNull();
  await act(async () => refresh.resolve({ ...snapshot, id: "refreshed" }));
});

it("holds a visible initial loader until its minimum time after the final dependency settles", async () => {
  vi.useFakeTimers();
  const query = deferred<ReviewSnapshot>();
  api.query.mockReturnValue(query.promise);
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} analysisClient={readyAnalysis()} />));
  await act(async () => vi.advanceTimersByTime(300));
  expect(loadingIndicator()).not.toBeNull();
  await act(async () => query.resolve(snapshot));
  expect(container.querySelector('.review-changes-content')?.hasAttribute('inert')).toBe(true);
  await act(async () => vi.advanceTimersByTime(399));
  expect(loadingIndicator()).not.toBeNull();
  await act(async () => vi.advanceTimersByTime(1));
  expect(loadingIndicator()).toBeNull();
  expect(container.querySelector('.review-changes-content')?.hasAttribute('inert')).toBe(false);
  expect(container.querySelector('button[aria-label="a"]')).not.toBeNull();
});

it("starts a fresh loading surface on conversation switch and ignores an old response", async () => {
  const old = deferred<ReviewSnapshot>(), next = deferred<ReviewSnapshot>();
  api.query.mockReturnValueOnce(old.promise).mockReturnValueOnce(next.promise);
  const analysisClient = readyAnalysis();
  const show = (taskId: string) => act(async () => root.render(<ReviewDock key={taskId} open taskId={taskId} running={false} onClose={() => {}} api={api} analysisClient={analysisClient} />));
  await show("task");
  await show("next");
  await act(async () => old.resolve(snapshot));
  expect(loadingIndicator()).not.toBeNull();
  expect(container.querySelector('button[aria-label="a"]')).toBeNull();
  await act(async () => next.resolve({ ...snapshot, taskId: "next", roots: [] }));
  expect(loadingIndicator()).toBeNull();
  expect(container.textContent).toContain("没有变更");
  expect(container.querySelector(".review-commit")).toBeNull();
});

it("exposes initial failures instead of leaving errors under the loading surface", async () => {
  api.query.mockRejectedValueOnce(new Error("Diff 读取失败"));
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} analysisClient={readyAnalysis()} />));
  expect(loadingIndicator()).toBeNull();
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("Diff 读取失败");
  const analysisClient = { bootstrap: async () => { throw new Error("offline"); }, turnReviews: async () => { throw new Error("offline"); } } as unknown as CoreClient;
  await act(async () => root.render(<ReviewPanel key="retry" taskId="task" running={false} onClose={() => {}} api={api} analysisClient={analysisClient} />));
  expect(loadingIndicator()).toBeNull();
  expect(container.textContent).toContain("无法读取审查状态");
  expect(container.querySelector('button[aria-label="a"]')).not.toBeNull();
});

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} }); vi.resetAllMocks();
  vi.mocked(tauri.invoke).mockRejectedValue(new Error("Native IPC unavailable in this fixture"));
  // jsdom has no top layer. This stub verifies lifecycle, not browser inertness.
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value: vi.fn(function (this: HTMLDialogElement) { this.open = true; }) });
  Object.defineProperty(HTMLDialogElement.prototype, "close", { configurable: true, value: vi.fn(function (this: HTMLDialogElement) { this.open = false; }) });
  api.query.mockResolvedValue(snapshot); api.file.mockResolvedValue(file); api.history.mockResolvedValue([]); api.branches.mockResolvedValue([]);
  api.preview.mockResolvedValue({ text: "working file", image: null });
  api.reveal.mockResolvedValue(undefined); api.ignore.mockResolvedValue(undefined);
  api.lease.mockResolvedValue(undefined);
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals();
  vi.useRealTimers();
  Reflect.deleteProperty(HTMLDialogElement.prototype, "showModal");
  Reflect.deleteProperty(HTMLDialogElement.prototype, "close");
});

it("refreshes the unopened file list when an agent finishes", async () => {
  await render(true);
  expect(api.query).toHaveBeenCalledTimes(1);
  api.query.mockResolvedValue({ ...snapshot, id: "finished" });
  await render(false);
  expect(api.query).toHaveBeenCalledTimes(2);
  expect(api.lease).toHaveBeenCalledWith(expect.objectContaining({ taskId: "task", snapshotId: "finished", release: false }));
  await render(false);
  expect(api.query).toHaveBeenCalledTimes(2);
});

it("shows one empty state in both file views and replaces it when changes arrive", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], files: [], statistics: {} }] });
  await render();
  expect(container.querySelectorAll(".review-empty-state")).toHaveLength(1);
  expect(container.querySelector(".review-empty-state h3")?.textContent).toBe("没有变更");
  await click(container.querySelector('[aria-label="以树形显示"]')!);
  expect(container.querySelectorAll(".review-empty-state")).toHaveLength(1);
  expect(container.querySelector(".review-root")).toBeNull();
  api.query.mockResolvedValue(snapshot);
  await click(container.querySelector('[aria-label="刷新"]')!);
  expect(container.querySelector(".review-empty-state")).toBeNull();
  expect(container.querySelector('button[aria-label="a"]')).not.toBeNull();
});

it.each([
  { error: "repository unavailable" },
  { excludedRoots: ["/repo/nested"] },
])("preserves directory diagnostics instead of showing an empty success state: %j", async diagnostic => {
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], files: [], statistics: {}, ...diagnostic }] });
  await render();
  expect(container.querySelector(".review-empty-state")).toBeNull();
  expect(container.querySelector(".review-root")).not.toBeNull();
});

it.each([false, true])("allows commit and push during an active conversation and still blocks duplicate operations (standalone=%s)", async standalone => {
  const commit = deferred<boolean>();
  const invoke = vi.mocked(tauri.invoke).mockReturnValueOnce(commit.promise).mockResolvedValue(true);
  try {
    await act(async () => root.render(<ReviewPanel taskId="task" running onClose={() => {}} api={api} analysisClient={readyAnalysis()} standalone={standalone} />));
    const input = container.querySelector<HTMLInputElement>('[aria-label="提交说明"]')!;
    expect(input.disabled).toBe(false);
    expect(container.querySelector<HTMLButtonElement>('[aria-label="选择提交说明 Agent"]')!.disabled).toBe(false);
    expect(container.querySelector<HTMLButtonElement>('[aria-label="生成提交说明"]')!.disabled).toBe(false);
    expect(container.textContent).not.toContain("请等待当前操作完成");
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "提交已完成的修改");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const submit = container.querySelector<HTMLButtonElement>(".review-commit-buttons > button:nth-child(2)")!;
    expect(submit.disabled).toBe(false);
    await click(submit);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_action", { taskId: "task", rootId: "repo", action: "commit", snapshotId: "snapshot", message: "提交已完成的修改", stageAll: false });
    expect(input.disabled).toBe(true);
    expect(submit.disabled).toBe(true);
    await click(submit);
    expect(invoke).toHaveBeenCalledOnce();
    await act(async () => commit.resolve(true));
    expect(invoke).toHaveBeenLastCalledWith("core_review_git_advance", { taskId: "task", operationId: expect.any(String) });
    expect(input.disabled).toBe(false);
    expect(container.querySelector('.review-commit [role="status"]')).toBeNull();

    const refresh = deferred<ReviewSnapshot>();
    api.query.mockReturnValueOnce(refresh.promise);
    await click(container.querySelector('[aria-label="刷新"]')!);
    expect(input.disabled).toBe(true);
    await act(async () => refresh.resolve(snapshot));
    expect(input.disabled).toBe(false);
  } finally {
    invoke.mockReset();
  }
});

it("keeps an open reader frozen when an agent finishes until explicitly refreshed", async () => {
  await render(true);
  await click(container.querySelector('button[aria-label="a"]')!);
  const reader = container.querySelector("code");
  const queries = api.query.mock.calls.length;
  await render(false);
  expect(api.query).toHaveBeenCalledTimes(queries);
  expect(container.querySelector("code")).toBe(reader);
  const refresh = container.querySelector('[aria-label="刷新"][data-changed]')!;
  expect(refresh).toBeDefined();
  await click(refresh);
  expect(api.query).toHaveBeenCalledTimes(queries + 1);
});

it("releases the old snapshot lease after refreshing to a new snapshot", async () => {
  await render();
  expect(api.lease).toHaveBeenCalledWith(expect.objectContaining({ taskId: "task", snapshotId: "snapshot", release: false }));
  api.query.mockResolvedValue({ ...snapshot, id: "next" });
  await click(container.querySelector('[aria-label="刷新"]')!);
  expect(api.lease).toHaveBeenCalledWith(expect.objectContaining({ taskId: "task", snapshotId: "snapshot", release: true }));
  expect(api.lease).toHaveBeenCalledWith(expect.objectContaining({ taskId: "task", snapshotId: "next", release: false }));
});

it("does not retain a previous comparison after a different scope fails to load", async () => {
  await render(); await click(container.querySelector("nav .review-file-entry")!);
  expect(container.querySelector("code")).not.toBeNull();
  api.query.mockRejectedValue(new Error("repository unavailable"));
  await select("比较模式", "unstaged");
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("无法读取变更");
  expect(container.textContent).not.toContain("repository unavailable");
  expect(container.querySelector("code")).toBeNull();
  expect(container.querySelector(".review-file-body code")).toBeNull();
  expect(container.textContent).not.toContain("丢弃此文件修改…");
});

it("removes obsolete diff and commit controls after an external commit on focus", async () => {
  await render(); await click(container.querySelector("nav .review-file-entry")!);
  api.query.mockResolvedValue({ ...snapshot, id: "probe", roots: [{ ...snapshot.roots[0], files: [] }] });
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  expect(api.query).toHaveBeenCalledWith(expect.objectContaining({ probe: true, taskId: "task" }));
  expect(api.query).toHaveBeenCalledTimes(3);
  expect(container.querySelector("code")).toBeNull();
  expect(container.querySelector(".review-file-entry")).toBeNull();
  expect(container.querySelector(".review-commit")).toBeNull();
  expect(api.file).toHaveBeenCalledTimes(1);
});





it("reads only the selected snapshot file and notifies without replacing on turn completion", async () => {
  await render(true);
  await click(container.querySelector("nav .review-file-entry")!);
  expect(api.file).toHaveBeenCalledWith("task", "snapshot", "repo", "a");
  expect(container.querySelector("code")?.textContent).toBe("new");
  await render(false);
  expect(container.querySelector('[aria-label="刷新"]')?.getAttribute("data-tooltip")).toBe("有新变化 · 刷新");
  expect(api.query).toHaveBeenCalledTimes(1);
});
it("swaps refreshed counts and selected body atomically", async () => {
  const version = { hash: "old", bytes: 1, kind: "file" as const, executable: false, binary: false };
  const initial: ReviewSnapshot = { ...snapshot, roots: [{ ...snapshot.roots[0], statistics: { a: { additions: 1, deletions: 0, binary: false, error: null } }, files: [{ path: "a", kind: "added", before: null, after: version }] }] };
  const next: ReviewSnapshot = { ...initial, id: "next", roots: [{ ...initial.roots[0], statistics: { a: { additions: 9, deletions: 0, binary: false, error: null } }, files: [{ ...initial.roots[0].files[0], after: { ...version, hash: "next", bytes: 999 } }] }] };
  api.query.mockResolvedValueOnce(initial).mockResolvedValue(next);
  await render(); await click(container.querySelector("nav .review-file-entry")!);
  let resolve!: (value: ReviewFile) => void;
  api.file.mockImplementationOnce(() => new Promise<ReviewFile>(done => { resolve = done; }));
  await click(container.querySelector('[aria-label="刷新"]')!);
  expect(container.querySelector(".review-total")?.textContent).toContain("+1");
  expect(container.querySelector(".review-file-metadata")).toBeNull();
  expect(container.querySelector("code")?.textContent).toBe("new");
  await act(async () => resolve({ ...file, text: { ...file.text!, after: ["refreshed\n"] } }));
  expect(container.querySelector(".review-total")?.textContent).toContain("+9");
  expect(container.querySelector(".review-file-metadata")).toBeNull();
  expect(container.querySelector("code")?.textContent).toBe("refreshed");
});

it("ignores a slow previous file response", async () => {
  let resolve!: (value: ReviewFile) => void;
  api.file.mockImplementationOnce(() => new Promise<ReviewFile>(r => { resolve = r; }));
  await render();
  await click(container.querySelectorAll("nav .review-file-entry")[0]);
  await click(container.querySelectorAll("nav .review-file-entry")[1]);
  expect(container.querySelector("code")?.textContent).toBe("new");
  await act(async () => resolve({ binary: true, text: null }));
  expect(container.querySelector("code")?.textContent).toBe("new");
  expect(container.textContent).not.toContain("二进制文件：");
});

it("requires an explicit historical commit and sends the selected merge parent", async () => {
  api.history.mockResolvedValue([{ id: "merge", subject: "Merge", parents: ["p1", "p2"], author: "A", date: "today" }]);
  await render();
  await select("比较模式", "committed");
  expect(api.query).toHaveBeenCalledTimes(1);
  await select("比较提交", "merge"); await select("父提交", "1");
  expect(api.query).toHaveBeenLastCalledWith({ taskId: "task", rootId: "repo", mode: "committed", reference: "merge", parent: 1 });
});

it("keeps expanded context and the mounted reader while refreshing the same file", async () => {
  const lines = Array.from({ length: 20 }, (_, i) => `line ${i}\n`);
  const contextFile: ReviewFile = { binary: false, text: { before: lines, after: lines, additions: 0, deletions: 0, blocks: [{ kind: "equal", oldStart: 0, oldEnd: 20, newStart: 0, newEnd: 20 }] } };
  api.file.mockResolvedValue(contextFile);
  await render(); await click(container.querySelector("nav .review-file-entry")!);
  await click(container.querySelector(".review-context button")!);
  const reader = container.querySelector(".review-text");
  expect(container.querySelectorAll("code")).toHaveLength(20);
  let resolve!: (value: ReviewSnapshot) => void;
  api.query.mockImplementationOnce(() => new Promise<ReviewSnapshot>(r => { resolve = r; }));
  await click(container.querySelector('[aria-label="刷新"]')!);
  expect(container.querySelector(".review-text")).toBe(reader);
  await act(async () => resolve({ ...snapshot, id: "next" }));
  expect(container.querySelector(".review-text")).toBe(reader);
  expect(container.querySelectorAll("code")).toHaveLength(20);
  expect(api.file).toHaveBeenLastCalledWith("task", "next", "repo", "a");
});













it("refreshes an externally committed file when reopening the retained reader", async () => {
  const dock = (open: boolean, taskId = "task") => act(async () => root.render(<ReviewDock key={taskId} taskId={taskId} open={open} running={false} onClose={() => {}} api={api} />));
  await dock(false);
  expect(api.query).not.toHaveBeenCalled();
  await dock(true);
  await click(container.querySelector('button[aria-label="a"]')!);
  const reader = container.querySelector("code");
  await dock(false);
  expect((container.querySelector("aside") as HTMLElement).hidden).toBe(true);
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], files: [] }] });
  await dock(true);
  expect(container.querySelector("code")).toBeNull();
  expect(api.query).toHaveBeenCalledTimes(3);
  expect(api.query).toHaveBeenCalledWith(expect.objectContaining({ probe: true }));
  await dock(false, "other-task");
  expect(container.querySelector("aside")).toBeNull();
  expect(api.lease).toHaveBeenCalledWith(expect.objectContaining({ taskId: "task", snapshotId: "snapshot", release: true }));
});

it("keeps loaded conversation panels and file readers across navigation without replaying loading", async () => {
  const analysisClient = readyAnalysis();
  const tasks = ["task", "other", "unvisited"].map(taskId => ({ taskId: taskId as TaskId, activePrimaryRun: null }));
  api.query.mockImplementation(async ({ taskId }) => ({ ...snapshot, id: `${taskId}-snapshot`, taskId }));
  const show = (selectedTaskId: string | null, open = true, available = tasks) => act(async () => root.render(
    <ConversationReviewDocks tasks={available} selectedTaskId={selectedTaskId} open={open} api={api} analysisClient={analysisClient} onClose={() => {}} />,
  ));
  await show("task");
  await click(container.querySelector('button[aria-label="a"]')!);
  const panel = container.querySelector("aside")!;
  const reader = panel.querySelector("code")!;
  const fileBody = panel.querySelector<HTMLElement>(".review-file-body")!;
  fileBody.scrollTop = 83;
  expect(loadingIndicator()).toBeNull();

  await show("other");
  expect(panel.hidden).toBe(true);
  expect(api.lease).not.toHaveBeenCalledWith(expect.objectContaining({ taskId: "task", snapshotId: "task-snapshot", release: true }));
  expect(api.file).toHaveBeenCalledTimes(1);
  const pendingProbe = deferred<ReviewSnapshot>();
  api.query.mockReturnValue(pendingProbe.promise);
  await show("task");
  expect(panel.hidden).toBe(false);
  expect(panel.querySelector("code")).toBe(reader);
  expect(fileBody.scrollTop).toBe(83);
  expect(loadingIndicator()).toBeNull();
  expect(api.file).toHaveBeenCalledTimes(1);
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ taskId: "task", probe: true }));
  expect(vi.mocked(analysisClient.bootstrap)).toHaveBeenCalledTimes(2);

  await show("unvisited", false);
  expect(container.querySelectorAll("aside:not([hidden])")).toHaveLength(0);
  expect(api.query.mock.calls.some(([query]) => query.taskId === "unvisited")).toBe(false);
  await show(null, false, tasks.filter(task => task.taskId !== tasks[0].taskId));
  expect(panel.isConnected).toBe(false);
  expect(api.lease).toHaveBeenCalledWith(expect.objectContaining({ taskId: "task", snapshotId: "task-snapshot", release: true }));
  await act(async () => pendingProbe.resolve(snapshot));
});

it("animates only opening the shared sidebar, not switching cached or unvisited conversations", async () => {
  const analysisClient = readyAnalysis();
  const tasks = ["task", "other", "unvisited"].map(taskId => ({ taskId: taskId as TaskId, activePrimaryRun: null }));
  api.query.mockImplementation(async ({ taskId }) => ({ ...snapshot, id: `${taskId}-snapshot`, taskId }));
  const show = (selectedTaskId: string, open: boolean) => act(async () => root.render(
    <StrictMode><ConversationReviewDocks tasks={tasks} selectedTaskId={selectedTaskId} open={open} api={api} analysisClient={analysisClient} onClose={() => {}} /></StrictMode>,
  ));
  const entering = () => container.querySelectorAll(".review-panel-entering");
  await show("task", false);
  await show("task", true);
  expect(entering()).toHaveLength(1);
  expect(container.querySelector("aside:not([hidden])")?.classList.contains("review-panel-entering")).toBe(true);
  await show("other", true);
  expect(entering()).toHaveLength(0);
  await show("task", true);
  expect(entering()).toHaveLength(0);
  await show("unvisited", true);
  expect(entering()).toHaveLength(0);
  await show("unvisited", false);
  await show("task", false);
  expect(entering()).toHaveLength(0);
  await show("task", true);
  expect(entering()).toHaveLength(1);
  // Even a switch before the opening animation finishes should not animate the new reader.
  await show("other", true);
  expect(entering()).toHaveLength(0);
});

it("keeps the all-directory scope when selecting a file or changing aggregate modes", async () => {
  const second = { ...snapshot.roots[0], root: { ...snapshot.roots[0].root, id: "second", path: "/private/work/second" }, base: "9e597f6b58f9dbc662c284f18b62fa4f1fd408" };
  api.query.mockResolvedValue({ ...snapshot, roots: [snapshot.roots[0], second] });
  await render();
  expect(container.querySelector<HTMLSelectElement>('[aria-label="比较目录"]')?.value).toBe("");
  expect(container.textContent).not.toContain("/private/work");
  expect(container.textContent).not.toContain(second.base);
  await click(container.querySelector('[data-root-id="second"] [aria-label="a"]')!);
  expect(api.file).toHaveBeenLastCalledWith("task", "snapshot", "second", "a");
  expect(api.query).toHaveBeenCalledTimes(1);
  await select("比较模式", "staged");
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ rootId: undefined, mode: "staged" }));
  expect(container.querySelectorAll(".review-root-heading")).toHaveLength(0);
  expect(container.querySelector(".review-view-toggle")?.getAttribute("aria-label")).toBe("以树形显示");
  await click(container.querySelector(".review-view-toggle")!);
  expect(container.querySelector(".review-view-toggle")?.getAttribute("aria-label")).toBe("以列表显示");
  expect(container.querySelectorAll(".review-root-heading")).toHaveLength(2);
  await click(container.querySelector(".review-view-toggle")!);
  expect(container.querySelectorAll(".review-root-heading")).toHaveLength(0);
  expect(container.querySelectorAll(".review-file-entry")).toHaveLength(4);
});

it("transfers the selected file and reading options to a detached reader", async () => {
  const detach = vi.fn();
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} onDetach={detach} api={api} />));
  await click(container.querySelector('[aria-label="a"]')!);
  await click(container.querySelector('[aria-label="左右对照"]')!);
  await click(container.querySelector('[aria-label="分离为独立窗口"]')!);
  expect(detach).toHaveBeenCalledTimes(1);
  const view = detach.mock.calls[0][0];
  expect(view.selection).toEqual({ rootId: "repo", path: "a" });
  expect(view.split).toBe(true);
  await act(async () => root.render(<StrictMode><ReviewPanel key="detached" taskId="task" running={false} initialView={view} onClose={() => {}} api={api} standalone /></StrictMode>));
  expect(container.querySelector('[aria-label="左右对照差异"]')).not.toBeNull();
  expect(api.file).toHaveBeenLastCalledWith("task", "snapshot", "repo", "a");
});

it("opens the exact file requested from a turn summary", async () => {
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} requestedPath="/repo/b" />));
  expect(api.file).toHaveBeenCalledWith("task", "snapshot", "repo", "b");
  expect(container.querySelector('.review-file-heading strong')?.textContent).toBe("repo/b");
});
it("does not substitute another file when a historical path is no longer in the workspace diff", async () => {
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} requestedPath="/repo/missing.ts" />));
  expect(api.file).not.toHaveBeenCalled();
  expect(container.textContent).toContain("当前工作区 Diff 中未找到该文件");
});
it("resolves identical basenames across multiple project directories without confusing roots", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [...snapshot.roots, { ...snapshot.roots[0], root: { id: "other", path: "/other", git: true, scopes: [""] } }] });
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} requestedPath="/other/a" />));
  expect(api.file).toHaveBeenCalledWith("task", "snapshot", "other", "a");
});

it("previews frozen image versions and keeps other binaries minimal", async () => {
  api.file.mockResolvedValue({ binary: true, text: null, beforeImage: 'data:image/png;base64,b2xk', afterImage: 'data:image/png;base64,bmV3' });
  await render();
  await click(container.querySelector('nav .review-file-entry')!);
  expect(container.querySelector('img[alt="修改前"]')?.getAttribute('src')).toBe('data:image/png;base64,b2xk');
  expect(container.querySelector('img[alt="修改后"]')?.getAttribute('src')).toBe('data:image/png;base64,bmV3');
  api.file.mockResolvedValue({ binary: true, text: null });
  await click(container.querySelector('nav [aria-label="b"]')!);
  expect(container.querySelector('img')).toBeNull();
  expect(container.textContent).toContain('此二进制文件暂不支持预览');
});

it("hides the commit composer for unmanaged directories", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], root: { ...snapshot.roots[0].root, git: false } }] });
  await render();
  expect(container.querySelector('[aria-label="提交说明"]')).toBeNull();
  expect(container.querySelector('[aria-label="选择 Agent 生成提交说明"]')).toBeNull();
  expect(container.querySelector(".review-commit")).toBeNull();
  expect(container.querySelector("dialog")).toBeNull();
});

it("switches workspace tabs while preserving the selected diff", async () => {
  await render();
  const tabs = container.querySelectorAll('[role="tab"]');
  expect(Array.from(tabs, tab => tab.getAttribute("aria-label"))).toEqual(["变更", "文件树", "运行"]);
  expect(tabs[0].getAttribute("aria-selected")).toBe("true");
  const fileButton = Array.from(container.querySelectorAll('nav button')).find(button => button.textContent?.includes("a"))!;
  await click(fileButton);
  const body = container.querySelector('.review-content') as HTMLElement;
  const content = body.textContent;
  await click(tabs[1]);
  expect(body.hidden).toBe(true);
  expect(container.querySelector('[aria-label="项目文件树"]')).not.toBeNull();
  await click(tabs[0]);
  expect(body.hidden).toBe(false);
  expect(body.textContent).toBe(content);
  expect(api.file).toHaveBeenCalledTimes(1);
  await act(async () => tabs[0].dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true })));
  expect(tabs[2].getAttribute("aria-selected")).toBe("true");
  expect(document.activeElement).toBe(tabs[2]);
});

it("omits clean roots from the flat list and shows one empty message when all roots are clean", async () => {
  const clean = { ...snapshot.roots[0], root: { ...snapshot.roots[0].root, id: "clean", path: "/clean" }, files: [], untracked: [] };
  api.query.mockResolvedValue({ ...snapshot, roots: [clean, snapshot.roots[0]] });
  await render();
  expect(container.querySelectorAll(".review-file-entry")).toHaveLength(2);
  expect(container.querySelector("nav")?.textContent).not.toContain("没有变更");
  api.query.mockResolvedValue({ ...snapshot, id: "clean-snapshot", roots: [clean, { ...clean, root: { ...clean.root, id: "also-clean" } }] });
  await click(container.querySelector('[aria-label="刷新"]')!);
  expect([...container.querySelectorAll("nav .review-empty-state h3")].filter(node => node.textContent === "没有变更")).toHaveLength(1);
});

it("reuses the open dock when a review request changes files or repeats the same file", async () => {
  const renderRequest = (requestedPath?: string, fileRequestId?: number) => act(async () => root.render(
    <ReviewDock open taskId="task" running={false} onClose={() => {}} api={api} requestedPath={requestedPath} fileRequestId={fileRequestId} />,
  ));
  await renderRequest();
  const panel = container.querySelector('.review-panel');
  await click(container.querySelector('[role="tab"][aria-label="文件树"]')!);
  await renderRequest('/repo/a', 1);
  expect(container.querySelector('.review-panel')).toBe(panel);
  expect(container.querySelector('[role="tab"][aria-label="变更"]')?.getAttribute('aria-selected')).toBe('true');
  expect(container.querySelector('.review-file-heading strong')?.textContent).toBe('repo/a');
  await renderRequest('/repo/b', 2);
  expect(container.querySelector('.review-panel')).toBe(panel);
  expect(container.querySelector('.review-file-heading strong')?.textContent).toBe('repo/b');
  const queries = api.query.mock.calls.length;
  await renderRequest('/repo/b', 3);
  expect(container.querySelector('.review-panel')).toBe(panel);
  expect(api.query).toHaveBeenCalledTimes(queries + 1);
  await renderRequest('/repo/missing.ts', 4);
  expect(container.querySelector('.review-file-heading')).toBeNull();
  expect(container.textContent).toContain('当前工作区 Diff 中未找到该文件');
});

const openMenu = async (path = "a") => act(async () => {
  container.querySelector<HTMLButtonElement>(`nav .review-file-entry[aria-label="${path}"]`)!.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 300, clientY: 200 }));
});
const menuItem = (label: string) => [...container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(button => button.textContent === label)!;
it("keeps source and fork readers docked while routing operations to the viewed workspace", async () => {
  container.className = "app-shell review-expanded";
  const source: ReviewWorkspace = { taskId: "task", name: "Main", conversationTitle: "Source", path: "/repo", branch: "main", head: "abcdef", changedFiles: 2, current: true, primary: true, git: true, running: false, error: null };
  const workspaceApi = { list: vi.fn(async () => [source, { ...source, taskId: "fork", name: "Fork", path: "/fork", current: false, primary: false }]) };
  const onOpenConversation = vi.fn();
  api.query.mockImplementation(async ({ taskId }: { taskId: string }) => ({ ...snapshot, taskId, id: `${taskId}-snapshot` }));
  const pending = deferred<void>(); api.index.mockReturnValueOnce(pending.promise);
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} analysisClient={readyAnalysis()} workspaceNavigation workspaceApi={workspaceApi} onOpenConversation={onOpenConversation} />));
  expect(container.querySelector(".app-shell > .review-panel")).not.toBeNull();
  await click(container.querySelector('button[aria-label="a"]')!);
  const originalReader = container.querySelector(".review-file-body");
  await click(container.querySelector('[aria-label="切换查看的工作区"]')!);
  await click(container.querySelectorAll('[role="menuitemradio"]')[1]);
  const viewed = () => container.querySelector('.app-shell > .review-panel:not([hidden])')!;
  expect(container.querySelectorAll(".app-shell > .review-panel")).toHaveLength(2);
  expect(container.querySelectorAll(".app-shell > .review-panel:not([hidden])")).toHaveLength(1);
  await click(viewed().querySelector('button[aria-label="b"]')!);
  expect(api.file).toHaveBeenLastCalledWith("fork", "fork-snapshot", "repo", "b");
  await act(async () => viewed().querySelector('[aria-label="b"]')!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 300, clientY: 200 })));
  await click(menuItem("暂存更改"));
  expect(api.index).toHaveBeenLastCalledWith("fork", "fork-snapshot", "repo", "b", "stage");
  expect(viewed().querySelector<HTMLButtonElement>('[aria-label="切换查看的工作区"]')!.disabled).toBe(true);
  await act(async () => pending.resolve());
  await click([...viewed().querySelectorAll("button")].find(button => button.textContent === "返回当前会话工作区")!);
  expect(viewed().querySelector(".review-file-body")).toBe(originalReader);
  expect(viewed().querySelector('.review-file-entry[aria-current="true"]')?.getAttribute("aria-label")).toBe("a");
  expect(onOpenConversation).not.toHaveBeenCalled();
});
it.each(["diff", "working", "head"])("closes the %s preview without closing the file list or reopening on a late response", async kind => {
  await render();
  const panel = container.querySelector(".review-panel");
  const pendingDiff = deferred<ReviewFile>();
  const pendingPreview = deferred<{ text: string; image: null }>();
  if (kind === "diff") api.file.mockReturnValueOnce(pendingDiff.promise);
  await click(container.querySelector('button[aria-label="a"]')!);
  if (kind !== "diff") {
    api.preview.mockReturnValueOnce(pendingPreview.promise);
    await openMenu(); await click(menuItem(kind === "head" ? "打开文件（HEAD）" : "打开文件"));
  }
  await click(container.querySelector('[aria-label="关闭文件预览"]')!);
  expect(container.querySelector(".review-panel")).toBe(panel);
  expect(container.querySelector(".review-file-body")).toBeNull();
  expect(container.querySelector('[aria-label="调整文件列表与差异区域宽度"]')).toBeNull();
  expect(container.querySelectorAll(".review-file-entry")).toHaveLength(2);
  expect(container.querySelector('.review-file-entry[aria-current="true"]')).toBeNull();
  expect(document.activeElement).toBe(container.querySelector('button[aria-label="a"]'));
  await act(async () => { pendingDiff.resolve(file); pendingPreview.resolve({ text: "late content", image: null }); });
  expect(container.querySelector(".review-file-body")).toBeNull();
  await click(container.querySelector('button[aria-label="a"]')!);
  expect(container.querySelector(".review-file-body code")?.textContent).toBe("new");
});
it("moves file operations into the context menu and stages its target instead of the selected file", async () => {
  await render(); await click(container.querySelector('button[aria-label="a"]')!);
  expect(container.textContent).not.toContain('恢复记录'); expect(container.querySelector('.review-actions')).toBeNull();
  await openMenu('b');
  expect(container.querySelectorAll('[role="menuitem"]')).toHaveLength(8);
  expect(container.textContent).not.toContain('查看 Git 历史');
  await click(menuItem('暂存更改'));
  expect(api.index).toHaveBeenCalledWith('task', 'snapshot', 'repo', 'b', 'stage');
  expect(api.query).toHaveBeenCalledTimes(2);
});
it("opens working and HEAD previews without replacing the diff panel", async () => {
  await render(); const panel = container.querySelector('.review-panel');
  await openMenu(); await click(menuItem('打开文件'));
  expect(api.preview).toHaveBeenLastCalledWith('task', 'snapshot', 'repo', 'a', 'working');
  expect(container.querySelector('.workspace-code')?.textContent).toContain('working file');
  await openMenu(); await click(menuItem('打开文件（HEAD）'));
  expect(api.preview).toHaveBeenLastCalledWith('task', 'snapshot', 'repo', 'a', 'head');
  expect(container.querySelector('.review-file-heading')?.textContent).toContain('(HEAD)');
  await openMenu('b'); await click(menuItem('打开更改'));
  expect(api.file).toHaveBeenLastCalledWith('task', 'snapshot', 'repo', 'b');
  expect(container.querySelector('.review-panel')).toBe(panel);
});
it("reveals and ignores the clicked untracked path, but disables ignore for tracked files", async () => {
  await render(); await openMenu(); expect(menuItem('加入 .gitignore').disabled).toBe(true);
  await openMenu('b'); await click(menuItem('在文件管理器中显示'));
  expect(api.reveal).toHaveBeenCalledWith('task', 'snapshot', 'repo', 'b');
  await openMenu('b'); await click(menuItem('加入 .gitignore'));
  expect(api.ignore).toHaveBeenCalledWith('task', 'snapshot', 'repo', 'b');
});
it("confirms paired discard, keeps errors in the dialog and blocks dismissal during execution", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], files: [{ path: 'new', previousPath: 'old', kind: 'renamed', before: null, after: null }] }] });
  let reject!: (error: Error) => void; api.discard.mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
  await render(); await openMenu('new'); await click(menuItem('丢弃更改…'));
  const dialog = container.querySelector('dialog')!;
  expect(dialog.open).toBe(true); expect(dialog.textContent).toContain('repo/old'); expect(dialog.textContent).toContain('repo/new');
  expect(api.discard).not.toHaveBeenCalled(); expect(api.batchFiles).not.toHaveBeenCalled(); expect(document.activeElement).toBe(dialog.querySelector('[data-dialog-title]')); 
  await click(dialog.querySelectorAll('button')[1]);
  expect(api.discard).toHaveBeenCalledWith({ taskId: 'task', snapshotId: 'snapshot', rootId: 'repo', path: 'new', confirmed: true });
  await act(async () => { dialog.dispatchEvent(new Event('cancel', { cancelable: true })); }); expect(container.querySelector('dialog')).toBe(dialog);
  await act(async () => { reject(new Error('文件已被其他程序修改')); }); expect(dialog.querySelector('[role="alert"]')?.textContent).toContain('文件已被其他程序修改');
  await click(dialog.querySelector('button')!); expect(container.querySelector('dialog')).toBeNull();
  expect(document.activeElement?.getAttribute('aria-label')).toBe('new');
});
it("disables mutations during runs while allowing read actions and reports stale index errors", async () => {
  await render(true); await openMenu(); expect(menuItem('暂存更改').disabled).toBe(true); expect(menuItem('丢弃更改…').disabled).toBe(true);
  await click(menuItem('在文件管理器中显示')); expect(api.reveal).toHaveBeenCalled();
  await render(false); api.index.mockRejectedValue('repository changed; refresh required');
  await openMenu(); await click(menuItem('暂存更改')); expect(container.querySelector('[role="alert"]')?.textContent).toContain('暂存更改失败');
  expect(container.textContent).not.toContain('refresh required');
});
it("supports keyboard context menus and skips disabled actions with arrow navigation", async () => {
  await render(); const file = container.querySelector<HTMLButtonElement>('button[aria-label="a"]')!;
  await act(async () => { file.dispatchEvent(new KeyboardEvent('keydown', { key: 'F10', shiftKey: true, bubbles: true })); });
  expect(document.activeElement).toBe(menuItem('打开更改'));
  await act(async () => { document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true })); });
  expect(document.activeElement).toBe(menuItem('在文件树中显示'));
  await act(async () => { document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); });
  expect(container.querySelector('[role="menu"]')).toBeNull(); expect(document.activeElement).toBe(file);
});

function directoryFixture(git = true) {
  const all = [snapshot.roots[0], {
    ...snapshot.roots[0], root: { id: "second", path: "/work/second", git, scopes: [""] },
    mode: git ? "uncommitted" as const : "session" as const,
    statistics: { a: { additions: 7, deletions: 3, binary: false, error: null } },
  }];
  api.query.mockImplementation(async (q) => ({
    ...snapshot, id: `${q.rootId ?? "all"}-${q.mode}-${q.reference ?? ""}`,
    roots: all.filter(r => (!q.rootId || r.root.id === q.rootId) && (!["staged", "unstaged"].includes(q.mode) || r.root.git)),
  }));
  return all;
}
const modeOptions = () => Array.from(container.querySelectorAll<HTMLOptionElement>('[aria-label="比较模式"] option'), o => o.value);

it("scopes files, counts and review input to the selected directory without cached roots leaking back", async () => {
  directoryFixture();
  const startProjectReview = vi.fn(async () => { throw new Error("test review accepted"); });
  const analysisClient = {
    bootstrap: async () => ({ installations: [{ providerId: "kimi", installationId: "k", status: "ready" }] }),
    reviewExecutors: async () => [{ providerId: "kimi", installationId: "k" }], turnReviews: async () => [], startProjectReview,
  } as unknown as import("@dogcodes/protocol").CoreClient;
  await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} analysisClient={analysisClient}/>));
  expect(modeOptions()).toEqual(["uncommitted", "unstaged", "staged"]);
  await select("比较目录", "second");
  expect(modeOptions()).toEqual(["uncommitted", "unstaged", "staged", "committed", "branch"]);
  expect(container.querySelectorAll('[data-root-id="repo"]')).toHaveLength(0);
  expect(container.querySelector(".review-total")?.textContent).toContain("+7");
  await click(container.querySelector('[aria-label="审查代码"]')!);
  expect(startProjectReview).toHaveBeenLastCalledWith(expect.objectContaining({ sourceSnapshot: "second-uncommitted-" }));
  await select("比较模式", "staged");
  await select("比较目录", "repo");
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ rootId: "repo", mode: "staged" }));
  expect(container.querySelector('[data-root-id="second"]')).toBeNull();
  expect(container.querySelector(".review-total")?.textContent ?? "").not.toContain("+7");
  await select("比较目录", "");
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ rootId: undefined, mode: "staged" }));
  expect(container.querySelectorAll(".review-root")).toHaveLength(2);
});

it("cascades mixed and unmanaged directories without offering unsupported Git comparisons", async () => {
  directoryFixture(false);
  await render();
  expect(container.querySelector('[aria-label="比较模式"]')?.textContent).toContain("当前改动");
  expect(container.querySelector('[aria-label="比较模式"]')?.textContent).toContain("已暂存（仅 Git）");
  expect(container.querySelector('[aria-label="比较目录"]')?.textContent).toContain("second（未使用 Git）");
  await select("比较模式", "staged");
  expect(container.querySelector('[data-root-id="second"]')).toBeNull();
  // The catalog must still include unmanaged directories after a Git-only query.
  await select("比较目录", "second");
  expect(container.querySelector('[aria-label="比较模式"]')).toBeNull();
  expect(container.querySelector(".review-controls")?.textContent).toContain("已记录修改");
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ rootId: "second", mode: "uncommitted" }));
  expect(container.querySelector('[data-root-id="repo"]')).toBeNull();
  await select("比较目录", "repo");
  expect(modeOptions()).toContain("branch");
  expect((container.querySelector('[aria-label="比较模式"]') as HTMLSelectElement).value).toBe("uncommitted");
});

it("keeps histories and branches repository-specific and discards late metadata from the previous directory", async () => {
  directoryFixture(); await render();
  await select("比较目录", "repo");
  let finish!: (commits: unknown[]) => void;
  api.history.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  await select("比较模式", "committed");
  api.history.mockResolvedValue([{ id: "second-commit", subject: "second history", parents: ["p"], author: "A", date: "today" }]);
  await select("比较目录", "second");
  expect(api.history).toHaveBeenLastCalledWith("task", "second");
  await act(async () => finish([{ id: "old", subject: "wrong history", parents: [] }]));
  expect(container.textContent).not.toContain("wrong history");
  await select("比较提交", "second-commit");
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ rootId: "second", reference: "second-commit" }));
  api.branches.mockResolvedValue([{ name: "main", commit: "head" }]);
  await select("比较模式", "branch"); await select("目标分支", "main");
  await select("比较目录", "repo");
  expect(api.branches).toHaveBeenLastCalledWith("task", "repo");
  expect((container.querySelector('[aria-label="目标分支"]') as HTMLSelectElement).value).toBe("");
  expect(container.querySelector(".review-root")).toBeNull();
  await select("比较目录", "");
  expect(modeOptions()).not.toContain("branch");
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ rootId: undefined, mode: "uncommitted", reference: undefined }));
});

it("hides redundant selectors for one unmanaged directory and keeps unavailable comparisons distinct from no changes", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], root: { ...snapshot.roots[0].root, git: false }, mode: "session", files: [], error: "baseline unavailable" }] });
  await render();
  expect(container.querySelector('[aria-label="比较目录"]')).toBeNull();
  expect(container.querySelector('[aria-label="比较模式"]')).toBeNull();
  expect(container.querySelector(".review-controls")?.textContent).toContain("已记录修改");
  expect(container.querySelector("nav")?.textContent).not.toContain("没有变更");
  expect(container.querySelector("nav [role=alert]")).not.toBeNull();
  expect((container.querySelector('[aria-label="审查代码"]') as HTMLButtonElement).disabled).toBe(true);
});

it("shows only session comparisons when every directory is unmanaged", async () => {
  const all = directoryFixture(false).map(r => ({ ...r, root: { ...r.root, git: false }, mode: "session" as const }));
  api.query.mockImplementation(async q => ({ ...snapshot, roots: all.filter(r => !q.rootId || r.root.id === q.rootId) }));
  await render();
  expect(container.querySelector('[aria-label="比较模式"]')).toBeNull();
  expect(container.querySelectorAll('[aria-label="比较目录"] option')).toHaveLength(3);
  await select("比较目录", "second");
  expect(container.querySelectorAll(".review-root")).toHaveLength(1);
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ rootId: "second", mode: "uncommitted" }));
  await select("比较目录", "");
  expect(container.querySelectorAll(".review-root")).toHaveLength(2);
  expect(container.querySelector('[aria-label="比较模式"]')).toBeNull();
});

it("preserves an explicit directory scope when detaching and restores all directories for a requested external file", async () => {
  directoryFixture(); const detach = vi.fn();
  const show = (requestedPath?: string) => act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} onDetach={detach} api={api} requestedPath={requestedPath}/>));
  await show(); await select("比较目录", "second");
  await select("比较模式", "staged");
  await click(container.querySelector('[aria-label="分离为独立窗口"]')!);
  expect(detach.mock.calls[0][0]).toMatchObject({ directoryId: "second", mode: "staged" });
  await show("/repo/b");
  expect((container.querySelector('[aria-label="比较目录"]') as HTMLSelectElement).value).toBe("");
  expect(api.file).toHaveBeenLastCalledWith("task", "all-uncommitted-", "repo", "b");
  const view = detach.mock.calls[0][0];
  await act(async () => root.render(<ReviewPanel key="detached" taskId="task" running={false} initialView={view} onClose={() => {}} api={api} standalone/>));
  expect((container.querySelector('[aria-label="比较目录"]') as HTMLSelectElement).value).toBe("second");
  expect(container.querySelector('[data-root-id="repo"]')).toBeNull();
  expect(api.query).toHaveBeenLastCalledWith(expect.objectContaining({ rootId: "second", mode: "staged" }));
});

it("opens a review finding in the correct repository without changing the comparison", async () => {
  const sharedPath = "src/application.properties";
  const multi: ReviewSnapshot = { ...snapshot, roots: ["one", "two"].map(id => ({ ...snapshot.roots[0], root: { ...snapshot.roots[0].root, id, path: `/repos/${id}` }, mode: "staged", files: [{ path: sharedPath, kind: "modified", before: null, after: null }] })) };
  api.query.mockResolvedValue(multi);
  const analysisClient = {
    bootstrap: vi.fn(async () => ({ installations: [] })), reviewExecutors: vi.fn(async () => []),
    turnReviews: vi.fn(async () => [{ workspaceReview: true, request: { taskId: "task", turnId: null, runId: "review", sourceSnapshot: "snapshot", executor: { providerId: "kimi", installationId: null }, kind: "code", retryOf: null }, status: "completed", createdAt: "2026-09-27T10:00:00Z", result: "", report: { structured: true, findings: [{ fileId: "f", path: sharedPath, rootId: "/repos/two", title: "上传失败", severity: "high", side: "after", startLine: 195, endLine: 196, description: "详情", suggestion: "建议", evidence: "", locationVerified: false }], files: [], warnings: [] } }]),
  } as unknown as import("@dogcodes/protocol").CoreClient;
  const scroll = vi.fn();
  const previousScroll = HTMLElement.prototype.scrollIntoView;
  HTMLElement.prototype.scrollIntoView = scroll;
  try {
    await act(async () => root.render(<ReviewPanel taskId="task" running={false} onClose={() => {}} api={api} analysisClient={analysisClient} initialView={{ rootId: "one", directoryId: "", mode: "staged", reference: "", parent: 0, split: false, filter: "hidden-by-filter", selection: null, roots: multi.roots.map(r => r.root) }}/>));
    const location = container.querySelector<HTMLButtonElement>(".review-location")!;
    const details = container.querySelector<HTMLElement>(".review-finding-details")!;
    expect(location.closest("[hidden]")).toBeNull();
    expect(details.hidden).toBe(true);
    await click(container.querySelector(".review-finding-toggle")!);
    expect(details.hidden).toBe(false);
    expect(location.textContent).toContain("application.properties");
    expect(location.textContent).not.toMatch(/repos|位置待核实|改动后/);
    expect(location.dataset.tooltip).toBe("/repos/two/src/application.properties");
    expect(location.disabled).toBe(false);
    await click(location);
    expect(api.file).toHaveBeenLastCalledWith("task", "snapshot", "two", sharedPath);
    expect(container.querySelector(".review-file-heading")?.textContent).toContain(`two/${sharedPath}`);
    expect(container.querySelector<HTMLSelectElement>('[aria-label="比较模式"]')?.value).toBe("staged");
    expect(container.querySelector('[aria-label="筛选文件"]')).toBeNull();
    expect(container.querySelector('[aria-label="提交说明"]')).not.toBeNull();
    expect(scroll).toHaveBeenCalled();
    await click(container.querySelector(".review-finding-toggle")!);
    expect(details.hidden).toBe(true);
  } finally { HTMLElement.prototype.scrollIntoView = previousScroll; }
});

it("hides the commit composer when only non-Git changes remain, and shows Git additions", async () => {
  const git = snapshot.roots[0];
  const unmanaged = { ...git, root: { ...git.root, id: "loose", git: false, path: "/loose" } };
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...git, files: [] }, unmanaged] });
  await render();
  expect(container.querySelector(".review-commit")).toBeNull();
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...git, files: [git.files[1]], untracked: [git.files[1].path] }] });
  await click(container.querySelector('[aria-label="刷新"]')!);
  expect(container.querySelector('[aria-label="提交说明"]')).not.toBeNull();
  expect(container.querySelector(".review-commit-repository")).toBeNull();
});

it.each([true, false])("shows the composer before opening a file when an earlier directory has no Git changes (git=%s)", async firstIsGit => {
  const changed = snapshot.roots[0];
  const first = { ...changed, root: { ...changed.root, id: "first", path: "/first", git: firstIsGit }, files: firstIsGit ? [] : changed.files };
  api.query.mockResolvedValue({ ...snapshot, roots: [first, changed] });
  await render();
  expect(container.querySelector('[aria-label="提交说明"]')).not.toBeNull();
  expect(container.querySelector('.review-commit-buttons > button')?.textContent).toContain("提交");
  expect(container.querySelector('[data-root-id="repo"]')?.getAttribute("data-active")).toBe("true");
  expect(container.querySelector('.review-file-body')).toBeNull();
  expect(api.file).not.toHaveBeenCalled();

  // Clicking a file must not remount the already usable composer.
  const composer = container.querySelector('.review-commit');
  await click(container.querySelector('[data-root-id="repo"] [aria-label="a"]')!);
  expect(container.querySelector('.review-commit')).toBe(composer);
});

it("retains the chosen repository when refreshing after its changes have been committed", async () => {
  const all = directoryFixture();
  await render();
  await click(container.querySelector('[data-root-id="second"] [aria-label="a"]')!);
  api.query.mockResolvedValue({ ...snapshot, roots: [all[0], { ...all[1], files: [] }] });
  await click(container.querySelector('[aria-label="刷新"]')!);
  await click(container.querySelector('[aria-label="以树形显示"]')!);
  expect(container.querySelector('[data-root-id="second"]')?.getAttribute("data-active")).toBe("true");
  expect(container.querySelector('[data-root-id="repo"]')?.getAttribute("data-active")).toBeNull();
});

const modifierClick = (element: Element, modifiers: MouseEventInit) => act(async () => { element.dispatchEvent(new MouseEvent("click", { bubbles: true, ...modifiers })); });
const selectAllFiles = () => act(async () => { container.querySelector('nav[aria-label="Review 文件列表"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "a", ctrlKey: true, bubbles: true })); });
it("stages exactly the selected files once and keeps modifier selection independent of preview", async () => {
  await render(); await click(container.querySelector('[aria-label="a"]')!); const calls = api.file.mock.calls.length;
  await modifierClick(container.querySelector('[aria-label="b"]')!, { shiftKey: true });
  expect(api.file).toHaveBeenCalledTimes(calls); expect(container.querySelectorAll('.review-file-entry[aria-pressed="true"]')).toHaveLength(2);
  await openMenu("a"); await click(batchMenuItem("暂存所选"));
  expect(api.index).toHaveBeenCalledExactlyOnceWith("task", "snapshot", "repo", ["a", "b"], "stage");
  expect(container.querySelectorAll('.review-file-entry[aria-pressed="true"]')).toHaveLength(0);
  expect(api.query).toHaveBeenCalledTimes(2);
});
it("uses a batch context menu for an existing selection and single-file menu for an unselected row", async () => {
  const expanded = { ...snapshot, roots: [{ ...snapshot.roots[0], files: [...snapshot.roots[0].files, { ...snapshot.roots[0].files[0], path: "c" }] }] };
  api.query.mockResolvedValue(expanded); await render(); await click(container.querySelector('[aria-label="a"]')!); await modifierClick(container.querySelector('[aria-label="b"]')!, { ctrlKey: true });
  await openMenu("a"); expect(document.querySelector('[role="menu"]')?.textContent).toContain("暂存所选（2 个文件）");
  expect(document.querySelector('[role="menu"]')?.textContent).toContain("丢弃所选更改");
  await openMenu("c"); expect(document.querySelector('[role="menu"]')?.textContent).toContain("打开更改");
});
it("reports cross-repository partial failure, preserves pending selection, and refreshes", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [...snapshot.roots, { ...snapshot.roots[0], root: { ...snapshot.roots[0].root, id: "two", path: "/two" } }] });
  api.index.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("索引已改变"));
  await render(); await selectAllFiles(); await openMenu("a"); await click(batchMenuItem("暂存所选"));
  expect(api.index).toHaveBeenCalledTimes(2); expect(container.textContent).toContain("已完成 2 个文件，其余未完成。");
  expect(container.textContent).toContain("索引已改变");
  expect(container.querySelectorAll('[data-root-id="repo"] [aria-pressed="true"]')).toHaveLength(0);
  expect(container.querySelectorAll('[data-root-id="two"] [aria-pressed="true"]')).toHaveLength(2);
  expect(api.query).toHaveBeenCalledTimes(2);
});
it("unstages the selection in staged mode and blocks repeated requests while pending", async () => {
  api.query.mockImplementation(async query => ({ ...snapshot, roots: [{ ...snapshot.roots[0], mode: query.mode }] }));
  await render(); await select("比较模式", "staged"); await selectAllFiles();
  const pending = deferred<void>(); api.index.mockReturnValue(pending.promise);
  await openMenu("a");
  const action = batchMenuItem("取消暂存所选");
  await click(action); await click(action); expect(api.index).toHaveBeenCalledExactlyOnceWith("task", "snapshot", "repo", ["a", "b"], "unstage");
  expect((container.querySelector('[aria-label="刷新"]') as HTMLButtonElement).disabled).toBe(true);
  await act(async () => pending.resolve());
});

it("keeps mixed non-Git selections visible but blocks a misleading partial stage", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [...snapshot.roots, { ...snapshot.roots[0], root: { ...snapshot.roots[0].root, id: "plain", git: false }, mode: "session" }] });
  await render(); await selectAllFiles();
  await openMenu("a");
  const action = batchMenuItem("暂存所选");
  expect(action.disabled).toBe(true); await click(action); expect(api.index).not.toHaveBeenCalled();
  expect(container.querySelectorAll('.review-file-entry[aria-pressed="true"]')).toHaveLength(4);
  await openMenu("a");
  expect(document.activeElement?.textContent).toBe("丢弃所选更改…");
  await act(async () => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(container.querySelector('[role="menu"]')).toBeNull();
  expect(container.querySelectorAll('.review-file-entry[aria-pressed="true"]')).toHaveLength(4);
});

const batchMenuItem = (label: string) => [...container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(button => button.textContent?.startsWith(label))!;
it("confirms the exact selection before discarding and prevents duplicate batch requests", async () => {
  await render(); await selectAllFiles(); await openMenu("a");
  await click(batchMenuItem("丢弃所选更改…"));
  let dialog = container.querySelector<HTMLDialogElement>('dialog[aria-label="丢弃所选文件更改"]')!;
  expect(dialog.open).toBe(true); expect(dialog.textContent).toContain("丢弃所选 2 个文件的更改");
  expect([...dialog.querySelectorAll("li")].map(item => item.textContent)).toEqual(["a", "b"]);
  expect(api.discard).not.toHaveBeenCalled(); expect(api.batchFiles).not.toHaveBeenCalled();
  await click([...dialog.querySelectorAll("button")].find(button => button.textContent === "取消")!);
  expect(api.discard).not.toHaveBeenCalled(); expect(api.batchFiles).not.toHaveBeenCalled();
  await openMenu("a"); await click(batchMenuItem("丢弃所选更改…"));
  dialog = container.querySelector<HTMLDialogElement>('dialog[aria-label="丢弃所选文件更改"]')!;
  const pending = deferred<{ completed: string[]; error: string | null }>(); api.batchFiles.mockReturnValueOnce(pending.promise);
  const confirm = [...dialog.querySelectorAll("button")].find(button => button.textContent === "丢弃 2 个文件的更改")!;
  await click(confirm); await click(confirm); expect(api.batchFiles).toHaveBeenCalledExactlyOnceWith({ taskId: "task", snapshotId: "snapshot", rootId: "repo", paths: ["a", "b"], action: "discard", confirmed: true });
  await act(async () => pending.resolve({ completed: ["a", "b"], error: null }));
  expect(container.querySelector('dialog[aria-label="丢弃所选文件更改"]')).toBeNull();
  expect(container.querySelectorAll('.review-file-entry[aria-pressed="true"]')).toHaveLength(0);
});
it("ignores all selected untracked files and retains unfinished files after a failure", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], untracked: ["a", "b"] }] });
  api.batchFiles.mockResolvedValueOnce({ completed: ["a"], error: "无法写入 .gitignore" });
  await render(); await selectAllFiles(); await openMenu("a"); await click(batchMenuItem("加入 .gitignore"));
  expect(api.batchFiles).toHaveBeenCalledExactlyOnceWith({ taskId: "task", snapshotId: "snapshot", rootId: "repo", paths: ["a", "b"], action: "ignore" });
  expect(container.textContent).toContain("已完成 1 个文件，其余未完成");
  expect(container.textContent).toContain("无法写入 .gitignore");
  expect(container.querySelector('[aria-label="a"]')?.getAttribute("aria-pressed")).toBe("false");
  expect(container.querySelector('[aria-label="b"]')?.getAttribute("aria-pressed")).toBe("true");
});
it("blocks bulk ignore for tracked files and bulk discard for staged changes", async () => {
  await render(); await selectAllFiles(); await openMenu("a");
  expect(batchMenuItem("加入 .gitignore").disabled).toBe(true);
  await click(batchMenuItem("加入 .gitignore")); expect(api.batchFiles).not.toHaveBeenCalled();
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], mode: "staged", untracked: [] }] });
  await select("比较模式", "staged"); await selectAllFiles(); await openMenu("a");
  expect(batchMenuItem("丢弃所选更改…").disabled).toBe(true);
  await click(batchMenuItem("丢弃所选更改…")); expect(api.discard).not.toHaveBeenCalled(); expect(api.batchFiles).not.toHaveBeenCalled();
});

const selectionMenu = () => openMenu("a");
it("previews the exact batch discard list and cancelling never writes files", async () => {
  await render(); await selectAllFiles(); await selectionMenu(); await click(menuItem("丢弃所选更改…"));
  const dialog = container.querySelector<HTMLDialogElement>('[aria-label="丢弃所选文件更改"]')!;
  expect(dialog.open).toBe(true); expect(dialog.textContent).toContain("2 个文件"); expect(dialog.textContent).toContain("Git 暂存区保持不变");
  expect([...dialog.querySelectorAll("li")].map(item => item.textContent)).toEqual(["a", "b"]);
  expect(document.activeElement).toBe(dialog.querySelector('[data-dialog-title]'));
  expect(api.batchFiles).not.toHaveBeenCalled();
  await click([...dialog.querySelectorAll("button")].find(button => button.textContent === "取消")!);
  expect(api.batchFiles).not.toHaveBeenCalled(); expect(container.querySelector("dialog")).toBeNull();
});
it("confirms batch discard, blocks dismissal while working, and keeps failed members selected", async () => {
  const pending = deferred<{ completed: string[]; error: string | null }>(); api.batchFiles.mockReturnValue(pending.promise);
  await render(); await selectAllFiles(); await selectionMenu(); await click(menuItem("丢弃所选更改…"));
  const dialog = container.querySelector<HTMLDialogElement>('[aria-label="丢弃所选文件更改"]')!;
  await click([...dialog.querySelectorAll("button")].find(button => button.textContent === "丢弃 2 个文件的更改")!);
  expect(api.batchFiles).toHaveBeenCalledExactlyOnceWith({ taskId: "task", snapshotId: "snapshot", rootId: "repo", paths: ["a", "b"], action: "discard", confirmed: true });
  await act(async () => { dialog.dispatchEvent(new Event("cancel", { cancelable: true })); }); expect(dialog.open).toBe(true);
  expect([...dialog.querySelectorAll("button")].every(button => button.disabled)).toBe(true);
  await act(async () => pending.resolve({ completed: ["a"], error: "b 在预览后有修改" }));
  expect(container.querySelector("dialog")).toBeNull(); expect(container.textContent).toContain("已完成 1 个文件，其余未完成");
  expect(container.querySelector('.review-file-entry[aria-label="a"]')?.getAttribute("aria-pressed")).toBe("false");
  expect(container.querySelector('.review-file-entry[aria-label="b"]')?.getAttribute("aria-pressed")).toBe("true");
  expect(api.query).toHaveBeenCalledTimes(2);
});
it("ignores only an all-untracked selection and refreshes after the batch", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], untracked: ["a", "b"] }] });
  api.batchFiles.mockResolvedValue({ completed: ["a", "b"], error: null });
  await render(); await selectAllFiles(); await selectionMenu(); await click(menuItem("加入 .gitignore（2 个文件）"));
  expect(api.batchFiles).toHaveBeenCalledExactlyOnceWith({ taskId: "task", snapshotId: "snapshot", rootId: "repo", paths: ["a", "b"], action: "ignore" });
  expect(api.query).toHaveBeenCalledTimes(2); expect(container.querySelectorAll('.review-file-entry[aria-pressed="true"]')).toHaveLength(0);
});
it("explains why tracked or historical selections cannot be ignored or discarded", async () => {
  await render(); await selectAllFiles(); await selectionMenu();
  expect(menuItem("加入 .gitignore（2 个文件）").disabled).toBe(true); expect(menuItem("加入 .gitignore（2 个文件）").dataset.tooltip).toContain("已跟踪文件");
  api.query.mockResolvedValue({ ...snapshot, id: "staged", roots: [{ ...snapshot.roots[0], mode: "staged" }] });
  await select("比较模式", "staged"); await selectAllFiles(); await selectionMenu();
  expect(menuItem("丢弃所选更改…").disabled).toBe(true); expect(menuItem("加入 .gitignore（2 个文件）").disabled).toBe(true);
});
it("refreshes externally committed files during the periodic check without a focus event", async () => {
  vi.useFakeTimers();
  try {
    await render(); await click(container.querySelector('button[aria-label="a"]')!);
    api.query.mockResolvedValue({ ...snapshot, id: "committed-elsewhere", roots: [{ ...snapshot.roots[0], files: [] }] });
    await act(async () => { await vi.advanceTimersByTimeAsync(10000); });
    expect(container.querySelector('.review-file-entry')).toBeNull();
    expect(container.querySelector('.review-file-body')).toBeNull();
    expect(container.querySelector('.review-commit')).toBeNull();
  } finally { vi.useRealTimers(); }
});
it("checks again after a quick return from another Git client", async () => {
  await render();
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  api.query.mockResolvedValue({ ...snapshot, id: "external", roots: [{ ...snapshot.roots[0], files: [] }] });
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  expect(api.query.mock.calls.filter(([query]) => query.probe)).toHaveLength(2);
  expect(container.querySelector('.review-file-entry')).toBeNull();
});
it("places the Git menu after the split-view toggle and keeps it usable when the working tree is clean", async () => {
  api.query.mockResolvedValue({ ...snapshot, roots: [{ ...snapshot.roots[0], files: [] }] });
  await render();
  const controls = [...container.querySelectorAll<HTMLButtonElement>('.review-sidebar-actions button')];
  expect(controls.slice(-2).map(button => button.getAttribute("aria-label"))).toEqual(["左右对照", "更多 Git 操作"]);
  await click(controls.at(-1)!);
  expect(container.querySelector('[role="menu"][aria-label="Git 操作"]')).not.toBeNull();
  expect([...container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].every(button => !button.disabled)).toBe(true);
});

it("keeps separate leases for two readers of the same coalesced snapshot", async () => {
  const show = (first: boolean) => act(async () => root.render(<>
    {first && <ReviewPanel key="first" taskId="task" running={false} onClose={() => {}} api={api} />}
    <ReviewPanel key="second" taskId="task" running={false} onClose={() => {}} api={api} />
  </>));
  await show(true);
  const claims = api.lease.mock.calls.map(([request]) => request).filter(request => !request.release);
  expect(claims).toHaveLength(2);
  expect(claims[0].leaseId).not.toBe(claims[1].leaseId);
  await show(false);
  expect(api.lease).toHaveBeenCalledWith({ ...claims[0], revision: 2, release: true });
  expect(api.lease).not.toHaveBeenCalledWith(expect.objectContaining({ leaseId: claims[1].leaseId, release: true }));
});
