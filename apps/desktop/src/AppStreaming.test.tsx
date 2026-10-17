// @vitest-environment jsdom
import { approvalRecoveryMocks } from "./testFixtures/approvalRecovery";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { CoreClient, EventEnvelope, EventStreamMessage, TimelineSnapshot } from "@dogcodes/protocol";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";
import { StreamingTimeline } from "./streamingTimeline";
import { approvalFixture } from "./testFixtures/approval";

const historyRender = vi.hoisted(() => vi.fn());
vi.mock("./ConversationHistory", async importOriginal => {
  const actual = await importOriginal<typeof import("./ConversationHistory")>();
  return { ...actual, ConversationHistory: (props: Parameters<typeof actual.ConversationHistory>[0]) => {
    historyRender(props);
    return <actual.ConversationHistory {...props} />;
  } };
});

let root: Root, host: HTMLDivElement;
let client: DemoCoreClient & Pick<CoreClient, "timelineSnapshot">;
let notify: (event: EventStreamMessage) => void;
let snapshot: Awaited<ReturnType<CoreClient["bootstrap"]>>;
let history: TimelineSnapshot;
const event = (offset: number, type = "message.delta", text = "字"): EventEnvelope => ({
  schemaVersion: 1, sequence: snapshot.lastSequence + offset, eventId: `evt_${offset}`,
  stream: { streamType: "task", streamId: snapshot.tasks[0].taskId }, streamVersion: 10 + offset,
  eventType: type, occurredAt: "2026-09-28T00:00:00Z", recordedAt: "2026-09-28T00:00:00Z", correlationId: "cor_test",
  causationId: null, source: { kind: "driver", providerId: "codex" }, vendorExtensions: null,
  payload: { taskId: snapshot.tasks[0].taskId, turnId: "trn_stream", itemId: "answer", text },
});
const deliver = async (events: EventEnvelope[], ms = 20) => act(async () => {
  events.forEach(data => notify({ type: "event", data }));
  await vi.advanceTimersByTimeAsync(ms);
});
beforeEach(async () => {
  localStorage.clear();
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.spyOn(performance, "now").mockImplementation(() => Date.now());
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(performance.now()), 16));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
  client = new DemoCoreClient(); localStorage.clear(); Object.assign(client, approvalRecoveryMocks()); snapshot = await client.bootstrap();
  snapshot.tasks[0].activePrimaryRun = { turnId: "trn_stream", runId: "run_stream", executor: snapshot.tasks[0].preferredExecutor, state: "running", startedAt: new Date().toISOString() };
  history = { items: await client.timeline(snapshot.tasks[0].taskId), lastSequence: snapshot.lastSequence };
  vi.spyOn(client, "bootstrap").mockImplementation(async () => structuredClone(snapshot));
  vi.spyOn(client, "pendingApprovals").mockResolvedValue([]);
  vi.spyOn(client, "cancelTurn").mockResolvedValue();
  vi.spyOn(client, "syncEvents").mockResolvedValue([]);
  vi.spyOn(client, "subscribe").mockImplementation(async (_, listener) => { notify = listener; return () => {}; });
  client.timelineSnapshot = vi.fn(async () => structuredClone(history));
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<App client={client} />));
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it("renders hundreds of native text deltas without fetching global state, history or approvals", async () => {
  const flush = vi.spyOn(StreamingTimeline.prototype, "flush");
  const sidebarRenders = historyRender.mock.calls.length;
  const reads = [vi.mocked(client.bootstrap).mock.calls.length, vi.mocked(client.timelineSnapshot!).mock.calls.length, vi.mocked(client.pendingApprovals).mock.calls.length];
  await deliver(Array.from({ length: 300 }, (_, i) => event(i + 1)));
  expect(host.querySelector(".assistant-message")?.textContent).toContain("字".repeat(300));
  expect(flush).toHaveBeenCalledTimes(1);
  expect(historyRender).toHaveBeenCalledTimes(sidebarRenders);
  expect([vi.mocked(client.bootstrap).mock.calls.length, vi.mocked(client.timelineSnapshot!).mock.calls.length, vi.mocked(client.pendingApprovals).mock.calls.length]).toEqual(reads);
  await deliver([event(300)]);
  expect(host.querySelector(".assistant-message")?.textContent?.match(/字/g)).toHaveLength(300);
  await act(async () => { host.querySelector<HTMLButtonElement>('[aria-label="停止"]')!.click(); });
  expect(client.cancelTurn).toHaveBeenCalledWith(expect.objectContaining({ streamVersion: 310 }), expect.stringMatching(/^cmd_/));
});
it("uses the latest received version when cancelling before the UI batch flushes", async () => {
  await deliver([event(1)], 0);
  expect(host.querySelector(".assistant-message")?.textContent ?? "").not.toContain("字");
  await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="停止"]')!.click());
  expect(client.cancelTurn).toHaveBeenCalledWith(expect.objectContaining({ streamVersion: 11 }), expect.stringMatching(/^cmd_/));
});

it("keeps an uncertain stop recoverable and never treats acknowledgement as task completion", async () => {
  vi.mocked(client.cancelTurn).mockRejectedValueOnce(new Error("connection lost"));
  await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="停止"]')!.click());
  expect(host.querySelector<HTMLButtonElement>('[aria-label="停止结果待核实"]')?.disabled).toBe(true);
  const query = [...host.querySelectorAll("button")].find(button => button.textContent === "核实停止请求")!;
  expect(query).toBeTruthy();
  await act(async () => query.click());
  expect(client.cancelTurn).toHaveBeenCalledTimes(1);
  expect(host.textContent).toContain("仍需等待任务实际退出");
  expect(host.querySelector('[aria-label="发送"]')).toBeNull();
});
it("uses the latest received version for an approval without refreshing task snapshots", async () => {
  vi.mocked(client.pendingApprovals).mockResolvedValue([{ ...approvalFixture, turnId: "trn_stream" }]);
  const respond = vi.spyOn(client, "respondApproval").mockResolvedValue();
  await deliver([event(1, "approval.requested")], 45);
  await deliver([event(2)], 0);
  await act(async () => host.querySelector<HTMLButtonElement>(".approval-primary")!.click());
  expect(respond).toHaveBeenCalledWith(expect.objectContaining({ streamVersion: 12 }), approvalFixture.approvalId, "native-once", { turnId: "trn_stream", requestedAtSequence: approvalFixture.requestedAtSequence }, undefined, expect.stringMatching(/^cmd_/));
});

it("updates a cached background conversation without rendering the selected conversation or sidebar", async () => {
  const backgroundId = "tsk_background" as const;
  snapshot.tasks.push({ ...snapshot.tasks[0], taskId: backgroundId, title: "后台会话" });
  vi.mocked(client.timelineSnapshot!).mockImplementation(async id => id === backgroundId
    ? { items: [], lastSequence: snapshot.lastSequence } : structuredClone(history));
  await act(async () => root.render(<App key="background" client={client} />));
  const select = async (title: string) => act(async () => {
    [...host.querySelectorAll<HTMLButtonElement>("button.task")].find(button => button.dataset.tooltip === title)!.click();
  });
  await select("后台会话");
  await select(snapshot.tasks[0].title);
  const reads = vi.mocked(client.timelineSnapshot!).mock.calls.length;
  const sidebarRenders = historyRender.mock.calls.length;
  const update = event(1, "message.delta", "后台追加");
  update.stream.streamId = backgroundId;
  update.payload = { ...(update.payload as object), taskId: backgroundId };
  await deliver([update]);
  expect(historyRender).toHaveBeenCalledTimes(sidebarRenders);
  expect(host.querySelector(".task-heading h1")?.textContent).toBe(snapshot.tasks[0].title);
  expect(host.textContent).not.toContain("后台追加");
  await select("后台会话");
  expect(host.querySelector(".assistant-message")?.textContent).toContain("后台追加");
  expect(client.timelineSnapshot).toHaveBeenCalledTimes(reads);
});
it("replays gap recovery events into the live message exactly once", async () => {
  vi.mocked(client.syncEvents).mockResolvedValueOnce([event(1, "message.delta", "你"), event(2, "message.delta", "好"), event(3, "message.delta", "！")]);
  await deliver([event(3, "message.delta", "！")]);
  expect(host.querySelector(".assistant-message")?.textContent).toContain("你好！");
  expect(client.syncEvents).toHaveBeenCalledWith(snapshot.lastSequence);
  expect(client.timelineSnapshot).toHaveBeenCalledTimes(1);
});
it("refreshes lifecycle changes and keeps deltas received during a slow canonical read", async () => {
  const initialBootstraps = vi.mocked(client.bootstrap).mock.calls.length;
  await deliver([event(1, "message.delta", "A")]);
  let finish!: (result: TimelineSnapshot) => void;
  vi.mocked(client.timelineSnapshot!).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  await deliver([event(2, "approval.resolved")], 45);
  await deliver([event(3, "message.delta", "B")]);
  const canonical = { ...history, lastSequence: snapshot.lastSequence + 2, items: [...history.items, {
    itemId: `message:${snapshot.tasks[0].taskId}:trn_stream:answer`, taskId: snapshot.tasks[0].taskId,
    turnId: "trn_stream" as const, kind: "assistant_message", text: "A", providerId: "codex", attachments: [],
    sequence: snapshot.lastSequence + 1, occurredAt: "2026-09-28T00:00:00Z",
  }] };
  await act(async () => finish(canonical));
  await act(async () => vi.advanceTimersByTimeAsync(32));
  expect(host.querySelector(".assistant-message")?.textContent).toContain("AB");
  expect(vi.mocked(client.bootstrap).mock.calls.length).toBeGreaterThan(initialBootstraps);
  expect(client.pendingApprovals).toHaveBeenCalledTimes(2);
});
it("reveals an incoming text burst over successive frames without extra reads", async () => {
  await deliver([event(1, "message.delta", "前文")]);
  const reads = vi.mocked(client.timelineSnapshot!).mock.calls.length;
  await deliver([event(2, "message.delta", "一二三四五六七八")]);
  expect(host.querySelector(".assistant-message")?.textContent).toBe("前文");
  await act(async () => vi.advanceTimersByTimeAsync(16));
  expect(host.querySelector(".assistant-message")?.textContent).toBe("前文一");
  await act(async () => vi.advanceTimersByTimeAsync(160));
  expect(host.querySelector(".assistant-message")?.textContent).toBe("前文一二三四五六七八");
  expect(client.timelineSnapshot).toHaveBeenCalledTimes(reads);
});
it("reconciles turn completion so the stop button and waiting state disappear", async () => {
  await deliver([event(1, "message.delta", "完成")]);
  snapshot.tasks[0].activePrimaryRun = null;
  history = { lastSequence: snapshot.lastSequence + 2, items: [...history.items, {
    itemId: `message:${snapshot.tasks[0].taskId}:trn_stream:answer`, taskId: snapshot.tasks[0].taskId,
    turnId: "trn_stream", kind: "assistant_message", text: "完成", providerId: "codex", attachments: [],
    sequence: snapshot.lastSequence + 1, occurredAt: "2026-09-28T00:00:00Z",
    turnCompletion: { startedAt: "2026-09-28T00:00:00Z", completedAt: "2026-09-28T00:00:02Z" },
  }] };
  await deliver([event(2, "turn.completed")], 60);
  expect(host.querySelector('[aria-label="停止"]')).toBeNull();
  expect(host.querySelector('.agent-pending')).toBeNull();
  expect(host.querySelector('.assistant-message')?.textContent).toContain("完成");
});
