// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, EventEnvelope, EventStreamMessage, TurnReviewRun } from "@dogcodes/protocol";
import { ReviewResult } from "./ReviewResult";

let container: HTMLDivElement, root: Root;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });
function fixture() {
  let receive!: (message: EventStreamMessage) => void;
  const stop = vi.fn();
  const client = { bootstrap: vi.fn(async () => ({ lastSequence: 10 })), subscribe: vi.fn(async (_: number, listener: typeof receive) => { receive = listener; return stop; }), syncEvents: vi.fn(async (): Promise<EventEnvelope[]> => []) };
  const run: TurnReviewRun = { workspaceReview: true, request: { taskId: "tsk_test", runId: "run_review", turnId: null, executor: { providerId: "kimi", installationId: null }, kind: "code", retryOf: null }, status: "running", createdAt: "2026-09-27T10:00:00Z", finishedAt: null, nativeSessionId: "native-review", sourceSequence: 1, sourceArtifact: null, inputSha256: "hash", coverage: "", result: "", error: null };
  const event = (sequence: number, eventType: string, payload: unknown) => ({ sequence, eventType, payload } as EventEnvelope);
  const emit = (data: EventEnvelope) => act(async () => receive({ type: "event", data }));
  return { client, core: client as unknown as CoreClient, run, emit, event, stop, receive: (message: EventStreamMessage) => receive(message) };
}
const read = { itemId: "read", data: { type: "tool", name: "Read", input: "src/a.ts", status: "running" } };
const message = { itemId: "reply", data: { type: "message", text: "已找到边界问题" } };
it("streams through main-conversation renderers, deduplicates mirrors and retains cancellation output", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-09-27T10:00:05Z"));
  const f = fixture();
  await act(async () => root.render(<ReviewResult client={f.core} run={f.run}/>));
  const payload = { taskId: "tsk_test", runId: "run_review", revision: 1, activities: [read, message, { ...message, itemId: "legacy", legacy: true }] };
  await f.emit(f.event(11, "review.activity", payload));
  expect(container.querySelectorAll(".assistant-message")).toHaveLength(1);
  expect(container.querySelector(".assistant-message")?.textContent).toBe("已找到边界问题");
  const work = container.querySelector<HTMLDetailsElement>(".turn-process")!;
  expect(work.open).toBe(false);
  expect(work.textContent).toContain("正在工作 0 分 5 秒");
  expect(container.querySelector(".conversation-activity")).toBeNull();
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(work.textContent).toContain("正在工作 0 分 6 秒");
  await act(async () => { work.open = true; work.dispatchEvent(new Event("toggle", { bubbles: true })); });
  expect(work.querySelector(".conversation-activity")?.textContent).toContain("src/a.ts");
  expect(work.querySelector(".assistant-message")).toBeNull();
  await f.emit(f.event(11, "review.activity", payload)); // replay duplicate
  expect(container.querySelectorAll(".assistant-message")).toHaveLength(1);
  await f.emit(f.event(12, "review.activity", { ...payload, runId: "run_foreign", revision: 8, activities: [{ ...message, data: { type: "message", text: "other review" } }] }));
  expect(container.textContent).not.toContain("other review");
  const cancelled = { ...f.run, finishedAt: "2026-09-27T10:00:08Z", status: "cancelled" as const, activityRevision: 1, activities: [read, message], result: message.data.text };
  await f.emit(f.event(13, "review.updated", { taskId: "tsk_test", review: cancelled }));
  expect(container.querySelector(".assistant-message")?.textContent).toBe(message.data.text);
  expect(container.textContent).toContain("结果未确认");
  await act(async () => root.render(<ReviewResult client={f.core} run={cancelled}/>));
  expect(f.stop).toHaveBeenCalledOnce();
  await act(async () => vi.advanceTimersByTimeAsync(5000));
  expect(work.textContent).toContain("已工作 0 分 8 秒");
  expect(container.textContent).toContain(message.data.text);
});
it("replaces live output with the finished report and keeps the execution expandable", async () => {
  const f = fixture();
  await act(async () => root.render(<ReviewResult client={f.core} run={f.run}/>));
  const report = { schemaVersion: 2, structured: true, findings: [], files: [], warnings: [] };
  await f.emit(f.event(11, "review.updated", { taskId: "tsk_test", review: { ...f.run, status: "completed", finishedAt: "2026-09-27T10:02:15Z", activityRevision: 2, activities: [read, message], result: '{"findings":[]}', report } }));
  expect(container.textContent).toContain("本次审查未报告问题");
  expect(container.querySelector(".review-activity")).toBeNull();
  const details = container.querySelector<HTMLDetailsElement>(".review-execution")!;
  expect(details.open).toBe(false);
  expect(container.textContent).not.toContain('{"findings":[]}');
  await act(async () => { details.open = true; details.dispatchEvent(new Event("toggle", { bubbles: true })); });
  expect(container.querySelector(".assistant-message")?.textContent).toBe(message.data.text);
  expect(details.textContent).not.toContain('{"findings":[]}');
  expect(details.querySelector<HTMLDetailsElement>(".turn-process")?.open).toBe(false);
  expect(details.querySelector(".turn-process-heading")?.textContent).toContain("已工作 2 分 15 秒");
});
it("shares the event stream, fills gaps, and reconnects from the last delivered sequence", async () => {
  vi.useFakeTimers();
  const f = fixture();
  await act(async () => root.render(<><ReviewResult client={f.core} run={f.run}/><ReviewResult client={f.core} run={{ ...f.run, request: { ...f.run.request, runId: "run_other" } }}/></>));
  expect(f.client.subscribe).toHaveBeenCalledTimes(1);
  const batch = f.event(11, "review.activity", { taskId: "tsk_test", runId: "run_review", revision: 1, activities: [message] });
  f.client.syncEvents.mockResolvedValueOnce([batch]);
  await f.emit(f.event(12, "unrelated", {}));
  expect(f.client.syncEvents).toHaveBeenCalledWith(10);
  expect(container.textContent).toContain(message.data.text);
  await act(async () => f.receive({ type: "error", data: { message: "offline", retryable: true, lastSequence: 12 } }));
  await act(async () => vi.advanceTimersByTimeAsync(500));
  expect(f.client.subscribe).toHaveBeenLastCalledWith(12, expect.any(Function));
});
it("restores failed output from saved activities without claiming running tools succeeded", async () => {
  const f = fixture();
  await act(async () => root.render(<ReviewResult client={f.core} run={{ ...f.run, status: "failed", activities: [read, message], result: message.data.text }}/>));
  const work = container.querySelector<HTMLDetailsElement>(".turn-process")!;
  expect(work.open).toBe(false);
  await act(async () => { work.open = true; work.dispatchEvent(new Event("toggle", { bubbles: true })); });
  expect(container.textContent).toContain("结果未确认");
  expect(container.querySelectorAll(".assistant-message")).toHaveLength(1);
  expect(f.client.subscribe).not.toHaveBeenCalled();
});
it("shows a friendly network failure, preserves progress, and folds the native diagnostic", async () => {
  const f = fixture();
  const error = 'Error: RetriableError: [internal] HTTP/2 keepalive ping timed out after 5000ms';
  const failed = {...f.run, status:'failed' as const, error, result:`正在检查调用关系。\n\n${error}`};
  await act(async () => root.render(<ReviewResult client={f.core} run={failed}/>));
  expect(container.querySelector('[role="alert"]')?.textContent).toBe('与审查 Agent 的连接中断，审查尚未完成。请检查网络或代理后重试。');
  expect(container.querySelector('.assistant-message')?.textContent).toBe('正在检查调用关系。');
  const details = container.querySelector<HTMLDetailsElement>('.review-failure details')!;
  expect(details.open).toBe(false);
  expect(details.querySelector('pre')?.textContent).toBe(error);
  expect(container.textContent).not.toContain('本次审查未报告问题');
});
it("never presents an incomplete empty report as a clean review", async () => {
  const f = fixture();
  await act(async () => root.render(<ReviewResult client={f.core} run={{...f.run, status:'failed', report:{schemaVersion:2,structured:true,findings:[],files:[],warnings:[]}}}/>));
  expect(container.querySelector('.review-summary.is-clean')).toBeNull();
  expect(container.textContent).not.toContain('本次审查未报告问题');
  expect(container.textContent).toContain('不能据此判断是否存在问题');
});
