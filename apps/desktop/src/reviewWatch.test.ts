// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { subscribeReviewChanges } from "./reviewWatch";
const channels = vi.hoisted(() => [] as { onmessage: (message: unknown) => void }[]);
const initialize = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage = (_: unknown) => {}; constructor() { initialize(); channels.push(this); } } }));
const stops: (() => void)[] = [];
beforeEach(() => { vi.useFakeTimers(); initialize.mockReset(); vi.mocked(invoke).mockReset().mockResolvedValue(undefined); channels.length = 0; });
afterEach(() => { stops.splice(0).forEach(stop => stop()); vi.useRealTimers(); });
it("shares one stream for multiple cards, refreshes after ready, and releases only the last subscriber", async () => {
  const first = vi.fn(), second = vi.fn();
  const a = subscribeReviewChanges("task", first), b = subscribeReviewChanges("task", second); stops.push(a, b);
  await Promise.resolve();
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "core_review_watch")).toHaveLength(1);
  first.mockClear(); second.mockClear(); a();
  channels[0].onmessage({ kind: "invalidated" });
  expect(first).not.toHaveBeenCalled(); expect(second).toHaveBeenCalledOnce();
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "core_review_unwatch")).toHaveLength(0);
  b(); expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "core_review_unwatch")).toHaveLength(1);
});
it("reconnects with a fresh subscription identity and ignores late events from the old stream", async () => {
  const refresh = vi.fn(), error = vi.fn();
  stops.push(subscribeReviewChanges("task", refresh, error)); await Promise.resolve();
  channels[0].onmessage({ kind: "error", message: "offline" });
  expect(error).toHaveBeenLastCalledWith(expect.stringContaining("正在重连"));
  await vi.advanceTimersByTimeAsync(1000);
  const connects = vi.mocked(invoke).mock.calls.filter(([name]) => name === "core_review_watch");
  expect(connects).toHaveLength(2); expect(connects[1][1]).not.toEqual(connects[0][1]);
  refresh.mockClear(); channels[0].onmessage({ kind: "invalidated" }); expect(refresh).not.toHaveBeenCalled();
  channels[1].onmessage({ kind: "invalidated" }); expect(refresh).toHaveBeenCalledOnce();
  expect(vi.mocked(invoke).mock.calls.every(([name]) => name === "core_review_watch" || name === "core_review_unwatch")).toBe(true);
});
it("does not resurrect a subscription canceled while connecting", async () => {
  let resolve!: () => void;
  vi.mocked(invoke).mockImplementation((name) => name === "core_review_watch" ? new Promise<void>(done => { resolve = done; }) : Promise.resolve());
  const refresh = vi.fn(), stop = subscribeReviewChanges("task", refresh); stops.push(stop);
  stop(); resolve(); await Promise.resolve();
  channels[0].onmessage({ kind: "invalidated" }); await vi.advanceTimersByTimeAsync(30000);
  expect(refresh).not.toHaveBeenCalled(); expect(channels).toHaveLength(1);
});

it("recovers a synchronous channel initialization failure without crashing the caller", async () => {
  initialize.mockImplementationOnce(() => { throw new Error("bridge unavailable"); });
  const refresh = vi.fn(), error = vi.fn();
  const stop = subscribeReviewChanges("task", refresh, error); stops.push(stop);
  expect(channels).toHaveLength(0);
  expect(error).toHaveBeenLastCalledWith(expect.stringContaining("bridge unavailable"));
  expect(refresh).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(1000);
  expect(channels).toHaveLength(1);
  expect(error).toHaveBeenLastCalledWith("");
  stop(); refresh.mockClear();
  await vi.advanceTimersByTimeAsync(30000);
  expect(channels).toHaveLength(1); expect(refresh).not.toHaveBeenCalled();
});
