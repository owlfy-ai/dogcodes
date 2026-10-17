// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ReviewWorkspaceBrowser, type ReviewWorkspace } from "./ReviewWorkspaceBrowser";

const main: ReviewWorkspace = { taskId: "main", name: "Project", conversationTitle: "原会话", path: "/project", branch: "main", head: "123456789", changedFiles: 2, current: true, primary: true, git: true, running: false, error: null };
const fork: ReviewWorkspace = { ...main, taskId: "fork", name: "Project · Fork", conversationTitle: "另一种实现", path: "/worktrees/fork", branch: "codex/fork-demo", changedFiles: 5, current: false, primary: false };
let root: Root, container: HTMLDivElement;
const api = { list: vi.fn<() => Promise<ReviewWorkspace[]>>() }, openConversation = vi.fn();
function Reader({ id, busy }: { id: string; busy: (value: boolean) => void }) {
  const [value, setValue] = useState(0);
  return <><button onClick={() => setValue(n => n + 1)}>{id}: {value}</button><button onClick={() => busy(true)}>开始操作</button><button onClick={() => busy(false)}>结束操作</button></>;
}
const render = (visible = true, request?: number) => act(async () => root.render(<ReviewWorkspaceBrowser taskId="main" running={false} visible={visible} api={api} onOpenConversation={openConversation} requestedPath={request ? "src/main.ts" : undefined} fileRequestId={request}>
  {target => <section data-reader hidden={!target.visible}>{target.header}<Reader id={target.taskId} busy={target.onBusy}/></section>}
</ReviewWorkspaceBrowser>));
const active = () => container.querySelector('[data-reader]:not([hidden])')!;
const button = (text: string) => [...active().querySelectorAll("button")].find(node => node.textContent === text)!;
const click = async (node: Element) => act(async () => (node as HTMLElement).click());
const menu = () => active().querySelector('[aria-label="切换查看的工作区"]')!;
const chooseFork = async () => { await click(menu()); await click(active().querySelectorAll('[role="menuitemradio"]')[1]); };
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  api.list.mockReset().mockResolvedValue([main, fork]); openConversation.mockReset();
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });

it("distinguishes workspaces and preserves the original reader when browsing a fork", async () => {
  await render(); await click(button("main: 0")); await chooseFork();
  expect(active().textContent).toContain("codex/fork-demo");
  expect(active().textContent).toContain("5 个文件有改动");
  expect(active().textContent).toContain("会话仍使用 Project");
  expect(openConversation).not.toHaveBeenCalled();
  await click(button("打开关联会话")); expect(openConversation).toHaveBeenCalledWith("fork");
  await click(button("返回当前会话工作区"));
  expect(button("main: 1")).toBeDefined();
  expect(active().textContent).toContain("当前会话使用");
});

it("locks workspace navigation for the duration of a Git operation", async () => {
  await render(); await chooseFork(); await click(button("开始操作"));
  expect((menu() as HTMLButtonElement).disabled).toBe(true);
  expect(button("返回当前会话工作区").disabled).toBe(true);
  expect(button("打开关联会话").disabled).toBe(true);
  await click(button("返回当前会话工作区")); expect(button("fork: 0")).toBeDefined();
  await click(button("结束操作")); await click(button("返回当前会话工作区"));
  expect(button("main: 0")).toBeDefined();
});

it("opens files requested by the chat in the chat's original workspace", async () => {
  await render(); await chooseFork(); await render(true, 1);
  expect(button("main: 0")).toBeDefined();
  await chooseFork(); await render(true, 2);
  expect(button("main: 0")).toBeDefined();
});

it("supports keyboard selection and Escape returns focus to the trigger", async () => {
  await render(); await click(menu());
  const choices = active().querySelectorAll('[role="menuitemradio"]');
  expect(document.activeElement).toBe(choices[0]);
  await act(async () => choices[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })));
  expect(document.activeElement).toBe(choices[1]);
  await act(async () => choices[1].dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(active().querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(menu());
});

it("shows errors and allows retry without blocking the current reader", async () => {
  api.list.mockRejectedValueOnce(new Error("读取失败")); await render();
  expect(active().querySelector('[role="alert"]')?.textContent).toContain("读取失败");
  expect(button("main: 0")).toBeDefined();
  await click(button("重试")); expect(active().querySelector('[role="alert"]')).toBeNull();
  expect(active().textContent).toContain("2 个文件有改动");
});

it("does not load hidden panels and discards responses after hiding", async () => {
  await render(false); expect(api.list).not.toHaveBeenCalled();
  let resolve!: (items: ReviewWorkspace[]) => void;
  api.list.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
  await render(); await render(false);
  await act(async () => resolve([{ ...main, name: "Stale" }]));
  await render(); expect(active().textContent).not.toContain("Stale");
  await chooseFork(); api.list.mockResolvedValue([main]);
  await click(active().querySelector('[aria-label="刷新工作区列表"]')!);
  expect(button("main: 0")).toBeDefined();
});
