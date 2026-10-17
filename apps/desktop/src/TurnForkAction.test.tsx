// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, ForkEnvironment, ForkRequest, TaskId, TimelineItem } from "@dogcodes/protocol";
import { TurnForkAction } from "./TurnForkAction";
import { ConversationContent } from "./ConversationActivity";

let root: Root, host: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); localStorage.clear();
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
const click = async (node: HTMLElement) => act(async () => node.click());
const trigger = () => host.querySelector<HTMLButtonElement>('[aria-label="从此处 Fork 对话"]')!;
const label = (text: string) => [...host.querySelectorAll<HTMLButtonElement>("button")].find(node => node.textContent === text)!;
const options = () => [...host.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
const key = async (node: Element, value: string) => act(async () => node.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true })));
async function mount(forkTask: CoreClient["forkTask"], onForked = vi.fn(), forkReceipt: CoreClient["forkReceipt"] = vi.fn(async () => null)) {
  await act(async () => root.render(<TurnForkAction client={{ forkTask, forkReceipt } as unknown as CoreClient} taskId="tsk_source" turnId="trn_old" onForked={onForked}/>));
  return onForked;
}
it.each<ForkEnvironment>(["workspace", "worktree"])("forks the selected historical turn into %s and opens the result", async environment => {
  const fork = vi.fn(async (request: ForkRequest) => request.newTaskId);
  const opened = await mount(fork);
  await click(trigger());
  expect(host.querySelector('[role="menu"]')?.textContent).toContain("从此处 Fork 对话");
  expect(options().map(option => option.textContent)).toEqual([
    "在当前工作区 Fork从本轮对话继续，沿用当前工作区", "在新 Worktree 中 Fork恢复本轮结束时的文件，在新 worktree 中继续",
  ]);
  await click(options()[environment === "workspace" ? 0 : 1]);
  expect(fork).toHaveBeenCalledExactlyOnceWith({ taskId: "tsk_source", turnId: "trn_old", environment, newTaskId: expect.stringMatching(/^tsk_[A-Z0-9]{26}$/) });
  expect(opened).toHaveBeenCalledWith(fork.mock.calls[0][0].newTaskId);
  expect(host.querySelector('[role="menu"]')).toBeNull();
});
it("prevents duplicate creation while pending and retries a lost reply with the same ID", async () => {
  let fail!: (error: Error) => void;
  const fork = vi.fn<(request: ForkRequest) => Promise<TaskId>>()
    .mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }))
    .mockImplementationOnce(async request => request.newTaskId);
  const opened = await mount(fork);
  await click(trigger()); await click(options()[0]);
  expect(trigger().disabled).toBe(true);
  expect(options().every(option => option.disabled)).toBe(true);
  expect(host.textContent).toContain("正在创建 Fork");
  await click(options()[1]); expect(fork).toHaveBeenCalledTimes(1);
  await act(async () => fail(new Error("服务连接中断")));
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("服务连接中断");
  expect(opened).not.toHaveBeenCalled();
  await click(label("核实并打开 Fork")); await click(label("重试原 Fork"));
  expect(fork.mock.calls[1][0]).toEqual(fork.mock.calls[0][0]);
  expect(opened).toHaveBeenCalledOnce();
});
it("supports keyboard navigation, Escape focus restoration and outside dismissal", async () => {
  await mount(vi.fn());
  await key(trigger(), "ArrowDown");
  expect(document.activeElement).toBe(options()[0]);
  await key(options()[0], "ArrowDown"); expect(document.activeElement).toBe(options()[1]);
  await key(options()[1], "Escape"); expect(document.activeElement).toBe(trigger());
  expect(trigger().getAttribute("aria-expanded")).toBe("false");
  await click(trigger());
  await act(async () => document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })));
  expect(host.querySelector('[role="menu"]')).toBeNull();
});
it("offers fork after each ended turn, including old turns while a later turn is running", async () => {
  const date = "2026-10-16T12:00:00Z";
  const items: TimelineItem[] = ["old", "new", "active"].map((id, index) => ({
    itemId: id, taskId: "tsk_source", turnId: `trn_${id}`, kind: "assistant_message", text: id, providerId: "codex",
    attachments: [], sequence: index + 1, occurredAt: date, ...(id !== "active" ? { turnCompletion: { startedAt: date, completedAt: date } } : {}),
  }));
  const fork = vi.fn(async (request: ForkRequest) => request.newTaskId);
  const opened = vi.fn();
  await act(async () => root.render(<ConversationContent items={items} client={{ forkTask: fork } as unknown as CoreClient} activeTurnId="trn_active" onSubagent={() => {}} renderEntry={item => <p>{item.text}</p>} onForked={opened}/>));
  expect(host.querySelectorAll('[aria-label="从此处 Fork 对话"]')).toHaveLength(2);
  await click(trigger()); await click(options()[0]);
  expect(fork.mock.calls[0][0].turnId).toBe("trn_old");
  expect(opened).toHaveBeenCalledOnce();
});
it.each([true, false])("distinguishes restored turn files from legacy worktrees (%s)", async restoredTurnSnapshot => {
  const item: TimelineItem = { itemId: "fork", taskId: "tsk_fork", turnId: null, providerId: null, kind: "fork_context", text: JSON.stringify({ environment: "worktree", restoredTurnSnapshot, text: "[]" }), attachments: [], sequence: 1, occurredAt: "2026-10-16T12:00:00Z" };
  await act(async () => root.render(<ConversationContent items={[item]} client={{} as CoreClient} onSubagent={() => {}} renderEntry={entry => <p>{entry.text}</p>}/>));
  expect(host.textContent).toContain(restoredTurnSnapshot ? "已恢复所选轮次结束时的文件" : "未恢复历史轮次的文件");
  if (restoredTurnSnapshot) expect(host.textContent).toContain("右侧“变更”");
});

it("keeps the original Fork across unmount and does not automatically resend", async () => {
  const fork = vi.fn<(request: ForkRequest) => Promise<TaskId>>().mockRejectedValueOnce(new Error("offline")).mockImplementation(async request => request.newTaskId);
  await mount(fork); await click(trigger()); await click(options()[1]);
  const original = fork.mock.calls[0][0];
  await act(async () => root.render(null)); await mount(fork);
  expect(fork).toHaveBeenCalledTimes(1);
  expect(host.textContent).toContain("有待核实的 Fork 请求");
  await click(trigger()); expect(options().slice(0, 2).every(option => option.disabled)).toBe(true);
  expect(document.activeElement).toBe(label("核实并打开 Fork"));
  await click(label("核实并打开 Fork")); await click(label("重试原 Fork"));
  expect(fork.mock.calls[1][0]).toEqual(original);
});
it("opens an accepted Fork after a lost reply without a second creation", async () => {
  const fork = vi.fn<(request: ForkRequest) => Promise<TaskId>>().mockRejectedValue(new Error("lost"));
  const receipt = vi.fn(async () => ({ request: fork.mock.calls[0][0], taskAvailable: true, completed: true }));
  const opened = await mount(fork, vi.fn(), receipt);
  await click(trigger()); await click(options()[0]);
  expect(fork).toHaveBeenCalledTimes(1); expect(opened).toHaveBeenCalledWith(fork.mock.calls[0][0].newTaskId);
  expect(host.textContent).not.toContain("待核实");
});
it("keeps an accepted Fork if opening its conversation fails and recovers it after remount", async () => {
  const fork = vi.fn(async (request: ForkRequest) => request.newTaskId);
  const receipt = vi.fn(async () => ({ request: fork.mock.calls[0][0], taskAvailable: true, completed: true }));
  const opened = vi.fn().mockRejectedValueOnce(new Error("navigation failed")).mockResolvedValue(undefined);
  await mount(fork, opened, receipt); await click(trigger()); await click(options()[0]);
  await act(async () => root.render(null)); await mount(fork, opened, receipt);
  await click(trigger()); await click(label("核实并打开 Fork"));
  expect(fork).toHaveBeenCalledTimes(1); expect(opened).toHaveBeenCalledTimes(2);
  expect(opened.mock.calls[1][0]).toBe(fork.mock.calls[0][0].newTaskId);
});
it("does not navigate from a request that finishes after its view has closed", async () => {
  let finish!: (id: TaskId) => void;
  const fork = vi.fn<(request: ForkRequest) => Promise<TaskId>>(() => new Promise(resolve => { finish = resolve; }));
  const opened = await mount(fork); await click(trigger()); await click(options()[0]);
  await act(async () => root.render(null));
  await act(async () => finish(fork.mock.calls[0][0].newTaskId));
  expect(opened).not.toHaveBeenCalled();
  await mount(fork); expect(host.textContent).toContain("有待核实的 Fork 请求");
});
it("refreshes Fork progress from canonical events without retrying or reopening automatically", async () => {
  const pending: ForkRequest = { taskId: "tsk_source", turnId: "trn_old", newTaskId: "tsk_target", environment: "worktree" };
  localStorage.setItem("dogcodes:pending-fork:tsk_source:trn_old", JSON.stringify(pending));
  let receive!: Parameters<CoreClient["subscribe"]>[1];
  const stop = vi.fn(), forkTask = vi.fn();
  const forkReceipt = vi.fn().mockResolvedValue({ request: pending, completed: false, taskAvailable: false, status: "running" });
  const subscribe = vi.fn<CoreClient["subscribe"]>().mockImplementation(async (_cursor, listener) => { receive = listener; return stop; });
  const client = { forkTask, forkReceipt, bootstrap: async () => ({ lastSequence: 5 }), subscribe } as unknown as CoreClient;
  await act(async () => root.render(<TurnForkAction client={client} taskId="tsk_source" turnId="trn_old"/>));
  await click(trigger());
  expect(host.textContent).toContain("Fork 正在后台执行");
  expect(host.textContent).not.toContain("重试原 Fork");
  forkReceipt.mockResolvedValue({ request: pending, completed: false, taskAvailable: false, status: "failed", stage: "restoring_worktrees", error: "第二个仓库权限不足" });
  await act(async () => receive({ type: "event", data: { sequence: 6, payload: { newTaskId: pending.newTaskId } } } as Parameters<typeof receive>[0]));
  expect(host.textContent).toContain("第二个仓库权限不足");
  expect(host.textContent).toContain("重试原 Fork");
  expect(forkTask).not.toHaveBeenCalled();
  await click(trigger()); expect(stop).toHaveBeenCalledOnce();
  const reads = forkReceipt.mock.calls.length;
  await act(async () => receive({ type: "event", data: { sequence: 7, payload: { newTaskId: pending.newTaskId } } } as Parameters<typeof receive>[0]));
  expect(forkReceipt).toHaveBeenCalledTimes(reads);
});
