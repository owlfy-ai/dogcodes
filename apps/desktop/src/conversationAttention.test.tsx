// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { CoreClient, EventEnvelope, TaskId, TaskSummary } from "@dogcodes/protocol";
import { strongestAttention, useConversationAttention } from "./conversationAttention";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => {}), isTauri: () => true }));
const eventApi = vi.hoisted(() => ({ handler: null as null | ((event: { payload: string }) => void), unlisten: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (_name: string, handler: (event: { payload: string }) => void) => { eventApi.handler = handler; return eventApi.unlisten; }) }));
let root: Root, host: HTMLDivElement;
let current: ReturnType<typeof useConversationAttention>;
const onOpen = vi.fn();
const pendingApprovals = vi.fn(async (): Promise<unknown[]> => []);
const client = { pendingApprovals } as unknown as CoreClient;
const task = (id: string, running = false) => ({ taskId: id as TaskId, title: `Title ${id}`, activePrimaryRun: running ? { runId: "run" } : null }) as unknown as TaskSummary;
const event = (eventType: string, id = "a", payload: unknown = {}) => ({ eventType, payload, stream: { streamType: "task", streamId: id } }) as unknown as EventEnvelope;
const notifications = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "notify_attention").map(([, args]) => args);
const badge = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "set_attention_badge").at(-1)?.[1];

function Probe(props: { tasks: TaskSummary[]; viewingTaskId: TaskId | null }) {
  current = useConversationAttention({ client, onOpen, ...props });
  return null;
}
const show = (tasks: TaskSummary[], viewing: string | null = null) => act(async () => root.render(<Probe tasks={tasks} viewingTaskId={viewing as TaskId | null} />));
const send = (value: EventEnvelope) => act(async () => { current.onEvent(value); });

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  vi.mocked(invoke).mockClear();
  onOpen.mockClear();
  pendingApprovals.mockReset().mockResolvedValue([]);
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

it("marks and announces a finished turn in a conversation you aren't looking at, until you open it", async () => {
  await show([task("a"), task("b")], "b");
  await send(event("turn.completed"));
  expect(current.attention).toEqual({ a: "completed" });
  expect(notifications()).toEqual([{ taskId: "a", title: "Title a", body: "这一轮已完成" }]);
  // Finished work doesn't count towards the badge.
  expect(badge()).toEqual({ count: 0 });
  await show([task("a"), task("b")], "a");
  expect(current.attention).toEqual({});
});

it("stays quiet for the conversation on screen, but not once the window is in the background", async () => {
  await show([task("a")], "a");
  await send(event("turn.failed"));
  expect(current.attention).toEqual({});
  expect(notifications()).toEqual([]);
  vi.mocked(document.hasFocus).mockReturnValue(false);
  await send(event("turn.failed"));
  expect(current.attention).toEqual({ a: "failed" });
  expect(notifications()).toHaveLength(1);
  expect(badge()).toEqual({ count: 1 });
  vi.mocked(document.hasFocus).mockReturnValue(true);
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  expect(current.attention).toEqual({});
});

it("follows the core for what's waiting, including approvals from before this window opened", async () => {
  pendingApprovals.mockResolvedValue([{ approvalId: "one" }]);
  await show([task("a", true)]);
  expect(pendingApprovals).toHaveBeenCalledWith("a");
  expect(current.attention).toEqual({ a: "approval" });
  expect(notifications()).toEqual([]);
  expect(badge()).toEqual({ count: 1 });

  pendingApprovals.mockResolvedValue([{ approvalId: "one" }, { approvalId: "two", question: { title: "?", schema: {} } }]);
  await send(event("approval.requested", "a", { question: { title: "?" } }));
  expect(current.attention).toEqual({ a: "question" });
  expect(notifications()).toEqual([{ taskId: "a", title: "Title a", body: "需要你回答后才能继续" }]);

  pendingApprovals.mockResolvedValue([]);
  await send(event("approval.resolved"));
  expect(current.attention).toEqual({});
  expect(badge()).toEqual({ count: 0 });
});

it("doesn't report waiting on a conversation whose run has ended", async () => {
  pendingApprovals.mockResolvedValue([{ approvalId: "one" }]);
  await show([task("a", true)]);
  await show([task("a", false)]);
  expect(current.attention).toEqual({});
});

it("remembers unseen results across restarts, and forgets them with the conversation or a new turn", async () => {
  await show([task("a"), task("b")]);
  await send(event("turn.failed"));
  await send(event("turn.completed", "b"));
  await act(async () => root.unmount());
  root = createRoot(host);
  await show([task("a"), task("b")]);
  expect(current.attention).toEqual({ a: "failed", b: "completed" });
  await send(event("turn.queued"));
  expect(current.attention).toEqual({ b: "completed" });
  await show([task("a")]);
  expect(current.attention).toEqual({});
  expect(localStorage.getItem("dogcodes.attention.unread.v1")).toBe("{}");
});

it("can be turned off without losing the marks in the app", async () => {
  await show([task("a")]);
  await act(async () => current.setNotifications(false));
  await send(event("turn.failed"));
  expect(notifications()).toEqual([]);
  expect(badge()).toEqual({ count: 0 });
  expect(current.attention).toEqual({ a: "failed" });
  expect(localStorage.getItem("dogcodes.notifications.v1")).toBe("false");
});

it("opens the conversation a clicked notification was about, if it still exists", async () => {
  await show([task("a")], "a");
  await act(async () => { eventApi.handler!({ payload: "gone" }); });
  expect(onOpen).not.toHaveBeenCalled();
  await act(async () => { eventApi.handler!({ payload: "a" }); });
  expect(onOpen).toHaveBeenCalledWith("a");
  await act(async () => root.unmount());
  expect(eventApi.unlisten).toHaveBeenCalled();
  root = createRoot(host);
});

it("opens the conversation whose notification launched the app, once conversations have loaded", async () => {
  vi.mocked(invoke).mockImplementation(async command => command === "take_attention_click" ? "a" : undefined);
  await act(async () => root.render(<Probe tasks={undefined as unknown as TaskSummary[]} viewingTaskId={null} />));
  expect(onOpen).not.toHaveBeenCalled();
  await show([task("a")]);
  expect(onOpen).toHaveBeenCalledTimes(1);
  expect(onOpen).toHaveBeenCalledWith("a");
  vi.mocked(invoke).mockImplementation(async () => {});
});

it("takes back a conversation's notification once it's been seen or answered", async () => {
  const withdrawn = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === "withdraw_attention").map(([, args]) => args);
  await show([task("a"), task("b")]);
  expect(withdrawn()).toEqual([]);
  await show([task("a"), task("b")], "a");
  expect(withdrawn()).toEqual([{ taskId: "a" }]);
  await send(event("approval.resolved", "b"));
  expect(withdrawn()).toEqual([{ taskId: "a" }, { taskId: "b" }]);
});

it("ranks what needs action above what can wait", () => {
  expect(strongestAttention(["completed", undefined, "failed", "approval"])).toBe("approval");
  expect(strongestAttention(["completed", "failed"])).toBe("failed");
  expect(strongestAttention([undefined])).toBeUndefined();
});
