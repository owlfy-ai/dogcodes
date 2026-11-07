// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ConversationMatch, CoreClient, ProjectSummary, TaskId, TaskSummary } from "@dogcodes/protocol";
import { ConversationSearch, mergeSearchResults, revealTimelineItem } from "./ConversationSearch";

let root: Root, host: HTMLDivElement;
const task = (id: string, title: string, updatedAt = "2026-01-01") => ({ taskId: id as TaskId, projectId: "p", title, updatedAt, preferredExecutor: { providerId: "codex" } }) as unknown as TaskSummary;
const match = (id: string, snippet = "the login form", fromUser = false): ConversationMatch => ({ taskId: id as TaskId, itemId: `item-${id}`, turnId: `turn-${id}`, fromUser, snippet });
const projects = [{ projectId: "p", displayName: "Repo" }] as unknown as ProjectSummary[];
const tasks = [task("a", "Login page"), task("b", "Payment"), task("c", "Docs")];
const search = vi.fn(async (_text: string): Promise<ConversationMatch[]> => []);
const onOpen = vi.fn(), onReveal = vi.fn();
const options = () => [...host.querySelectorAll<HTMLElement>('[role="option"]')];
const field = () => host.querySelector<HTMLInputElement>("input")!;
const type = async (value: string) => {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field(), value);
    field().dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => { await vi.advanceTimersByTimeAsync(250); });
};
const press = (key: string, target: EventTarget = field(), init: KeyboardEventInit = {}) => act(async () => { target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init })); });
const render = (client: Partial<CoreClient> = { searchConversations: search }) => act(async () => root.render(
  <ConversationSearch client={client as CoreClient} tasks={tasks} projects={projects} shortcutKey="Meta" onOpen={onOpen} onReveal={onReveal}><p id="list">list</p></ConversationSearch>));

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  search.mockReset().mockResolvedValue([]); onOpen.mockClear(); onReveal.mockClear();
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });

it("lists title matches first, then message matches, each conversation once", () => {
  const results = mergeSearchResults(tasks, " LOGIN ", [match("b"), match("a"), match("gone")]);
  expect(results.map(result => result.task.taskId)).toEqual(["a", "b"]);
  expect(results[0].message?.itemId).toBe("item-a");
  expect(mergeSearchResults(tasks, "  ", [match("a")])).toEqual([]);
});

it("waits for typing to pause and ignores an answer to an older search", async () => {
  await render();
  let finishOld!: (value: ConversationMatch[]) => void;
  search.mockImplementationOnce(() => new Promise(resolve => { finishOld = resolve; }));
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(field(), "lo");
    field().dispatchEvent(new Event("input", { bubbles: true }));
  });
  expect(search).not.toHaveBeenCalled();
  expect(options().map(option => option.textContent)).toEqual(["Login pageRepo"]);
  await act(async () => { await vi.advanceTimersByTimeAsync(250); });
  search.mockResolvedValueOnce([match("c", "new answer")]);
  await type("login");
  await act(async () => finishOld([match("b", "old answer")]));
  expect(host.textContent).toContain("new answer");
  expect(host.textContent).not.toContain("old answer");
});

it("hides the list while searching without unmounting it, and Escape brings it back", async () => {
  await render();
  const list = host.querySelector("#list")!;
  await type("login");
  expect(list.isConnected).toBe(true);
  expect((list.parentElement as HTMLElement).style.display).toBe("none");
  await press("Escape");
  expect(field().value).toBe("");
  expect((list.parentElement as HTMLElement).style.display).toBe("contents");
  expect(host.querySelector('[role="option"]')).toBeNull();
});

it("moves through results by keyboard and opens the one chosen at its message", async () => {
  search.mockResolvedValue([match("b", "the login form", true)]);
  await render();
  await type("login");
  expect(options()).toHaveLength(2);
  expect(options()[1].textContent).toContain("你: the login form");
  expect(options()[1].querySelector("mark")?.textContent).toBe("login");
  await press("ArrowDown");
  expect(options()[1].getAttribute("aria-selected")).toBe("true");
  expect(field().getAttribute("aria-activedescendant")).toBe(options()[1].id);
  await press("Enter");
  expect(onOpen).toHaveBeenCalledWith("b", { itemId: "item-b", turnId: "turn-b" });
  await press("ArrowUp");
  await press("Enter");
  expect(onOpen).toHaveBeenLastCalledWith("a", undefined);
  expect(field().value).toBe("login");
});

it("treats what's typed as plain text", async () => {
  await render();
  await type("(a+");
  expect(host.querySelector('[role="status"]')?.textContent).toBe("没有找到相关对话");
});

it("still offers title matches when messages can't be searched", async () => {
  search.mockRejectedValue(new Error("busy"));
  await render();
  await type("login");
  expect(options()).toHaveLength(1);
  expect(host.textContent).toContain("无法搜索消息内容");
  await render({});
  await type("payment");
  expect(options()).toHaveLength(1);
  expect(host.textContent).not.toContain("无法搜索消息内容");
});

it("focuses on its shortcut, opening a collapsed sidebar, but not behind a dialog", async () => {
  vi.stubGlobal("navigator", { ...navigator, platform: "MacIntel" });
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callback(0); return 0; });
  await render();
  await press("f", window, { metaKey: true, shiftKey: true });
  expect(onReveal).toHaveBeenCalledTimes(1);
  expect(document.activeElement).toBe(field());
  field().blur();
  const dialog = document.createElement("dialog"); dialog.setAttribute("open", ""); document.body.append(dialog);
  await press("f", window, { metaKey: true, shiftKey: true });
  expect(onReveal).toHaveBeenCalledTimes(1);
  dialog.remove();
});

it("opens a turn's collapsed steps to reach a message, then marks it", async () => {
  const done = vi.fn();
  const details = document.createElement("details"); details.dataset.turnProcess = "turn"; document.body.append(details);
  revealTimelineItem("item", "turn", done);
  expect(details.open).toBe(true);
  expect(done).not.toHaveBeenCalled();
  const message = document.createElement("div"); message.dataset.timelineItem = "item"; details.append(message);
  await vi.advanceTimersByTimeAsync(60);
  expect(message.classList.contains("timeline-item-revealed")).toBe(true);
  expect(done).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(2300);
  expect(message.classList.contains("timeline-item-revealed")).toBe(false);
  details.remove();
});

it("gives up on a message that never appears, and can be cancelled", async () => {
  const done = vi.fn();
  revealTimelineItem("missing", null, done);
  await vi.advanceTimersByTimeAsync(2100);
  expect(done).toHaveBeenCalledTimes(1);
  const cancelled = vi.fn();
  revealTimelineItem("missing", null, cancelled)();
  await vi.advanceTimersByTimeAsync(2100);
  expect(cancelled).not.toHaveBeenCalled();
});
