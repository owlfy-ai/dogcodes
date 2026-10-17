import { RunOperationRejected } from "./projectRunOperations";
// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ProjectRunPanel } from "./ProjectRunPanel";
import { WorkspaceTabs } from "./WorkspaceTabs";
import type { ProjectRunApi, ProjectRunCatalog } from "./projectRunClient";
vi.mock("./TerminalDock", () => ({ TerminalPane: ({ visible }: { visible: boolean }) => <div data-terminal hidden={!visible} />, terminalClient: {} }));
let root: Root, element: HTMLDivElement;
beforeEach(() => { HTMLDialogElement.prototype.showModal = function () { this.open = true; }; HTMLDialogElement.prototype.close = function () { this.open = false; }; vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); element = document.createElement("div"); document.body.append(element); root = createRoot(element); });
afterEach(async () => { await act(async () => root.unmount()); element.remove(); vi.unstubAllGlobals(); vi.useRealTimers(); });
const catalog: ProjectRunCatalog = { projectId: "project", generation: "v1", platform: "macos", shell: "POSIX sh", executor: { providerId: "codex", installationId: "one" }, directories: [
  { path: "/project/web", status: "ready", error: null, commands: [{ id: "dev", label: "运行本地调试", command: "npm run dev", cwd: ".", source: "package.json" }, { id: "deploy", label: "上线部署", command: "./deploy.sh", cwd: ".", source: "deploy.sh" }] },
  { path: "/project/api", status: "ready", error: null, commands: [{ id: "api", label: "启动服务", command: "go run ./cmd/server", cwd: ".", source: "README.md" }] },
] };
function fixture() {
  const stop = vi.fn(async () => {});
  const api: ProjectRunApi = { watch: vi.fn(() => () => {}), hasPending: () => false, discardPending: () => {}, retryPending: async () => catalog, mutate: async () => catalog, get: vi.fn(async () => catalog), edit: vi.fn(async () => catalog), analyze: vi.fn(async () => catalog), executors: vi.fn(async () => [catalog.executor!]), list: vi.fn(async () => []), restore: vi.fn(info => ({ id: info.id, api: {} as never, stop })), launch: vi.fn(() => ({ id: crypto.randomUUID(), api: {} as never, stop })) };
  return { api, stop };
}
const click = async (label: string) => act(async () => {
  const button = [...element.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === label || b.getAttribute("aria-label") === label)!;
  expect(button).toBeDefined(); button.click();
});
it("places Run after Files with keyboard tab navigation", async () => {
  const change = vi.fn(); await act(async () => root.render(<WorkspaceTabs value="files" onChange={change} />));
  expect([...element.querySelectorAll('[role="tab"]')].map(e => e.getAttribute("aria-label"))).toEqual(["变更", "文件树", "运行"]);
  await act(async () => element.querySelector('[aria-label="文件树"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true })));
  expect(change).toHaveBeenCalledWith("run"); expect(document.activeElement?.getAttribute("aria-label")).toBe("运行");
});
it("lists per-directory commands without running any and launches only the chosen recipe", async () => {
  const { api, stop } = fixture();
  const show = (visible: boolean) => act(async () => root.render(<ProjectRunPanel taskId="task" visible={visible} api={api} />));
  await show(true);
  expect(api.launch).not.toHaveBeenCalled();
  expect(element.textContent).not.toContain("macOS · POSIX sh");
  const deploy = [...element.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent === "上线部署")!;
  expect(deploy.dataset.tooltip).toContain("./deploy.sh"); expect(deploy.dataset.tooltip).toContain("/project/web");
  await click("启动服务");
  expect(api.launch).toHaveBeenCalledWith("task", catalog, 1, "api", expect.any(Function));
  await click("启动服务运行中"); expect(api.launch).toHaveBeenCalledTimes(1);
  await show(false); await show(true);
  expect(element.querySelectorAll('[data-terminal]')).toHaveLength(1); expect(api.launch).toHaveBeenCalledTimes(1);
  await click("停止"); expect(stop).toHaveBeenCalledTimes(1);
  const onExit = vi.mocked(api.launch).mock.calls[0][4];
  await act(async () => onExit());
  expect(element.textContent).toContain("启动服务 · 已结束");
  await click("启动服务"); expect(api.launch).toHaveBeenCalledTimes(2);
  await click("关闭输出 启动服务");
  expect(element.querySelectorAll('[data-terminal]')).toHaveLength(1);
  expect(element.querySelector('[data-terminal]')?.hasAttribute("hidden")).toBe(false);
  await click("关闭输出 启动服务");
  expect(element.querySelector('[aria-label="运行输出"]')).toBeNull();
});
it("shows analysis, empty and error states and reanalyzes with the chosen Agent", async () => {
  const { api } = fixture();
  vi.mocked(api.get).mockResolvedValue({ ...catalog, directories: [ { ...catalog.directories[0], commands: [], status: "ready" }, { ...catalog.directories[1], commands: [], status: "error", error: "Agent 额度已耗尽" } ] });
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  expect(element.textContent).toContain("未找到适合当前系统的运行命令"); expect(element.textContent).toContain("Agent 额度已耗尽");
  await click("重新分析运行命令"); expect(api.analyze).toHaveBeenCalledWith("task", catalog.executor, undefined, expect.any(String));
  expect(api.launch).not.toHaveBeenCalled();
});
it("polls pending analyses but pauses when the panel is hidden", async () => {
  vi.useFakeTimers(); const { api } = fixture();
  vi.mocked(api.get).mockResolvedValue({ ...catalog, directories: [{ ...catalog.directories[0], status: "analyzing", commands: [] }] });
  const show = (visible: boolean) => act(async () => root.render(<ProjectRunPanel taskId="task" visible={visible} api={api} />));
  await show(true); expect(element.textContent).toContain("正在分析运行方式");
  await act(async () => vi.advanceTimersByTime(1200)); expect(api.get).toHaveBeenCalledTimes(2);
  await show(false); await act(async () => vi.advanceTimersByTime(12000)); expect(api.get).toHaveBeenCalledTimes(2);
});

it("keeps the chosen Agent across polling, excludes automatic and offline options, and supports menu dismissal", async () => {
  vi.useFakeTimers();
  const { api } = fixture();
  const claude = { providerId: "claude-code", installationId: "two" };
  vi.mocked(api.executors).mockResolvedValue([{ providerId: "fake", installationId: null }, catalog.executor!, claude]);
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  await click("运行命令分析 Agent");
  expect(element.querySelector(".agent-picker-menu .agent-icon.small")).not.toBeNull();
  expect(element.textContent).not.toContain("自动选择可用 Agent");
  expect(element.textContent).not.toContain("离线模式");
  await click("Claude Code");
  await act(async () => vi.advanceTimersByTime(10000));
  await click("重新分析运行命令");
  expect(api.analyze).toHaveBeenCalledWith("task", claude, undefined, expect.any(String));
  await click("运行命令分析 Agent");
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(element.querySelector(".agent-picker-menu")).toBeNull();
  expect(document.activeElement?.getAttribute("aria-label")).toBe("运行命令分析 Agent");
});


it("edits, pins and deletes items without launching them and confirms overwriting customizations", async () => {
  const { api } = fixture();
  let current = structuredClone(catalog);
  vi.mocked(api.get).mockImplementation(async () => current);
  vi.mocked(api.edit).mockImplementation(async (_task, generation, index, id, edit) => {
    expect(generation).toBe(current.generation);
    current = structuredClone(current);
    const directory = current.directories[index];
    const command = directory.commands.find(c => c.id === id)!;
    if (edit.action === "rename") { command.label = edit.label; directory.userModified = true; }
    if (edit.action === "delete") { directory.commands = directory.commands.filter(c => c.id !== id); directory.userModified = true; }
    if (edit.action === "pin") command.pinned = edit.pinned;
    current.generation += "-edit";
    return current;
  });
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  await click("重命名 运行本地调试");
  const input = element.querySelector<HTMLInputElement>('[aria-label="运行项名称"]')!;
  expect(document.activeElement).toBe(input);
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "自定义调试");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click("保存名称");
  expect(element.querySelector('[aria-label="重命名 自定义调试"]')).not.toBeNull();
  await click("固定 上线部署");
  expect(element.querySelector(".project-run-command")?.textContent).toBe("上线部署");
  expect(element.querySelector('[aria-label="取消固定 上线部署"]')?.getAttribute("aria-pressed")).toBe("true");
  await click("取消固定 上线部署");
  expect(element.querySelector('[aria-label="固定 上线部署"]')).not.toBeNull();
  await click("删除 启动服务");
  expect(element.textContent).not.toContain("启动服务");
  expect(api.launch).not.toHaveBeenCalled();
  await click("重新分析运行命令");
  expect(element.querySelector("dialog[open]")?.textContent).toContain("覆盖未固定");
  expect(element.querySelector("dialog")?.textContent).toContain("已固定的运行项及其名称、命令将保留");
  expect(api.analyze).not.toHaveBeenCalled();
  await click("取消");
  expect(element.querySelector("dialog")).toBeNull();
  await click("重新分析运行命令");
  await click("重新分析");
  expect(api.analyze).toHaveBeenCalledWith("task", catalog.executor, current.generation, current.generation);
});

it("cancels renaming and rejects empty names without saving", async () => {
  const { api } = fixture();
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  await click("重命名 运行本地调试");
  const input = element.querySelector<HTMLInputElement>('[aria-label="运行项名称"]')!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "   ");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(element.querySelector<HTMLButtonElement>('[aria-label="保存名称"]')?.disabled).toBe(true);
  await act(async () => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(element.querySelector('[aria-label="运行项名称"]')).toBeNull();
  expect(api.edit).not.toHaveBeenCalled();
});

it("requests fresh confirmation if another window edits the saved list", async () => {
  const { api } = fixture();
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  vi.mocked(api.get).mockResolvedValue({ ...catalog, generation: "newer", directories: [{ ...catalog.directories[0], userModified: true }] });
  vi.mocked(api.analyze).mockRejectedValueOnce(new RunOperationRejected("confirmation_required", "需要确认覆盖"));
  await click("重新分析运行命令");
  expect(element.querySelector("dialog[open]")).not.toBeNull();
  await click("重新分析");
  expect(api.analyze).toHaveBeenLastCalledWith("task", catalog.executor, "newer", "newer");
});

it("centers one empty state for all empty directories and reuses analysis without launching", async () => {
  const { api } = fixture();
  const empty = { ...catalog, directories: catalog.directories.map(directory => ({ ...directory, commands: [] })) };
  vi.mocked(api.get).mockResolvedValue(empty);
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  expect(element.querySelectorAll(".project-run-directories--empty .review-empty-state")).toHaveLength(1);
  expect(element.querySelector(".project-run-directory")).toBeNull();
  expect(element.querySelector(".review-empty-state h3")?.textContent).toBe("暂无运行命令");
  await click("重新分析");
  expect(api.analyze).toHaveBeenCalledWith("task", catalog.executor, undefined, expect.any(String));
  expect(api.launch).not.toHaveBeenCalled();
});

it("uses compact hints for empty directories alongside runnable commands", async () => {
  const { api } = fixture();
  vi.mocked(api.get).mockResolvedValue({ ...catalog, directories: [{ ...catalog.directories[0], commands: [] }, catalog.directories[1]] });
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  expect(element.querySelector(".project-run-directories--empty")).toBeNull();
  expect(element.querySelectorAll(".review-empty-state--compact")).toHaveLength(1);
  await click("启动服务");
  expect(api.launch).toHaveBeenCalledWith("task", expect.anything(), 1, "api", expect.any(Function));
});

it.each(["analyzing", "error"] as const)("does not present %s as an empty analysis result", async status => {
  const { api } = fixture();
  vi.mocked(api.get).mockResolvedValue({ ...catalog, directories: [{ ...catalog.directories[0], commands: [], status, error: status === "error" ? "分析失败" : null }] });
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  expect(element.querySelector(".review-empty-state")).toBeNull();
  expect(element.querySelector(status === "error" ? '[role="alert"]' : ".project-run-analyzing")).not.toBeNull();
});

it("explains missing directories and does not offer an unusable analysis action", async () => {
  const { api } = fixture();
  vi.mocked(api.get).mockResolvedValue({ ...catalog, directories: [] });
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  expect(element.querySelector(".review-empty-state h3")?.textContent).toBe("尚未关联项目目录");
  expect(element.querySelector(".review-empty-action")).toBeNull();
  expect(element.querySelector<HTMLButtonElement>('[aria-label="重新分析运行命令"]')?.disabled).toBe(true);
});

it("guides users with no available Agent and still confirms before regenerating cleared items", async () => {
  const { api } = fixture();
  vi.mocked(api.get).mockResolvedValue({ ...catalog, directories: [{ ...catalog.directories[0], commands: [], userModified: true }] });
  vi.mocked(api.executors).mockResolvedValue([]);
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  expect(element.querySelector(".review-empty-state")?.textContent).toContain("连接可用的 Agent");
  expect(element.querySelector(".review-empty-action")).toBeNull();
  vi.mocked(api.executors).mockResolvedValue([catalog.executor!]);
  await act(async () => root.render(<ProjectRunPanel key="with-agent" taskId="task" visible api={api} />));
  await click("重新分析");
  expect(element.querySelector("dialog[open]")).not.toBeNull();
  expect(api.analyze).not.toHaveBeenCalled();
  await click("取消");
  expect(element.querySelector("dialog[open]")).toBeNull();
});

it("restores background project processes and hides output without stopping or relaunching", async () => {
  const { api, stop } = fixture();
  vi.mocked(api.list).mockResolvedValue([{ id: "restored", epoch: "epoch", projectId: "project", taskId: "task", commandId: "api", directoryIndex: 1, label: "启动服务", cwd: "/project/api", command: "go run ./cmd/server", status: "running", exitCode: null, error: null, outputEnd: 0, createdAt: 1 }]);
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  expect(api.restore).toHaveBeenCalledTimes(1); expect(api.launch).not.toHaveBeenCalled();
  expect(element.querySelectorAll('[data-terminal]')).toHaveLength(1);
  await click("关闭输出 启动服务");
  expect(stop).not.toHaveBeenCalled(); expect(element.querySelectorAll('[data-terminal]')).toHaveLength(0);
  await click("启动服务运行中");
  expect(element.querySelectorAll('[data-terminal]')).toHaveLength(1);
  expect(api.launch).not.toHaveBeenCalled();
});

it("blocks new mutations while a saved operation is unresolved and exposes explicit recovery", async () => {
  const { api } = fixture(); let pending = true;
  vi.spyOn(api, "hasPending").mockImplementation(() => pending);
  const retry = vi.spyOn(api, "retryPending").mockImplementation(async () => { pending = false; return catalog; });
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  expect(element.textContent).toContain("上次操作的结果尚未核实");
  expect(element.querySelector<HTMLButtonElement>('[aria-label="重新分析运行命令"]')!.disabled).toBe(true);
  await click("重新核实／重试原操作");
  expect(retry).toHaveBeenCalledExactlyOnceWith("task");
  expect(api.analyze).not.toHaveBeenCalled(); expect(api.edit).not.toHaveBeenCalled();
  expect(element.textContent).not.toContain("上次操作的结果尚未核实");
});
it("keeps an analysis rejection visible after the catalog refresh", async () => {
  const { api } = fixture();
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  vi.mocked(api.analyze).mockRejectedValueOnce(new RunOperationRejected("executor_unavailable", "分析 Agent 当前不可用或已禁用"));
  const reads = vi.mocked(api.get).mock.calls.length;
  await click("重新分析运行命令");
  expect(vi.mocked(api.get).mock.calls.length).toBeGreaterThan(reads);
  expect(element.querySelector('[role="alert"]')?.textContent).toContain("分析 Agent 当前不可用或已禁用");
  expect(element.querySelector("dialog[open]")).toBeNull();
});
it("refreshes a conflicting edit without replaying it and uses the new version on the next gesture", async () => {
  const { api } = fixture();
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  vi.mocked(api.get).mockResolvedValue({ ...catalog, generation: "newer" });
  vi.mocked(api.edit).mockRejectedValueOnce(new RunOperationRejected("version_conflict", "运行项已更新，请刷新后重试"));
  await click("固定 上线部署");
  expect(api.edit).toHaveBeenCalledTimes(1);
  expect(element.querySelector('[role="alert"]')?.textContent).toContain("运行项已更新");
  await click("固定 上线部署");
  expect(api.edit).toHaveBeenLastCalledWith("task", "newer", 0, "deploy", { action: "pin", pinned: true });
});
it("does not mistake an unstructured failure for overwrite authorization", async () => {
  const { api } = fixture();
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  api.discardPending = vi.fn();
  vi.mocked(api.analyze).mockRejectedValueOnce("run_commands_confirmation_required");
  await click("重新分析运行命令");
  expect(element.querySelector("dialog[open]")).toBeNull();
  expect(api.discardPending).not.toHaveBeenCalled();
  expect(element.querySelector('[role="alert"]')?.textContent).toContain("分析运行命令失败");
});
it("refreshes shared recipes immediately on invalidation and releases the watch when hidden", async () => {
  const { api } = fixture();
  let changed!: () => void;
  const stop = vi.fn();
  vi.mocked(api.watch).mockImplementation((_task, refresh) => { changed = refresh; return stop; });
  const show = (visible: boolean) => act(async () => root.render(<ProjectRunPanel taskId="task" visible={visible} api={api} />));
  await show(true);
  vi.mocked(api.get).mockResolvedValue({ ...catalog, generation: "another-window", directories: [{ ...catalog.directories[0], commands: [{ ...catalog.directories[0].commands[0], label: "另一窗口已改名" }] }] });
  await act(async () => changed());
  expect(element.textContent).toContain("另一窗口已改名");
  expect(api.launch).not.toHaveBeenCalled();
  await show(false); expect(stop).toHaveBeenCalledOnce();
  const calls = vi.mocked(api.get).mock.calls.length;
  await act(async () => changed()); expect(api.get).toHaveBeenCalledTimes(calls);
});
it("coalesces invalidations arriving during a read and does not publish the stale response", async () => {
  vi.useFakeTimers();
  const { api } = fixture();
  let changed!: () => void, resolve!: (value: ProjectRunCatalog) => void;
  vi.mocked(api.watch).mockImplementation((_task, refresh) => { changed = refresh; return () => {}; });
  vi.mocked(api.get).mockImplementationOnce(() => new Promise(done => { resolve = done; }));
  await act(async () => root.render(<ProjectRunPanel taskId="task" visible api={api} />));
  await act(async () => { changed(); changed(); resolve({ ...catalog, generation: "stale" }); });
  expect(element.textContent).not.toContain("运行本地调试");
  await act(async () => vi.advanceTimersByTime(0));
  expect(api.get).toHaveBeenCalledTimes(2);
  expect(element.textContent).toContain("运行本地调试");
});
