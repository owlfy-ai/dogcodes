// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { QuotaSnapshot } from "@dogcodes/protocol";
import { ContextUsageRing, contextUsage } from "./ContextUsageRing";
import { Tooltips } from "./Tooltips";

const base: QuotaSnapshot = { windows: [], collectedAtUnixMs: 1, staleAfterUnixMs: 2, confidence: "provider_reported", source: "native_push", parserVersion: "test" };
let root: Root, container: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.useFakeTimers();
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([{}] as unknown as DOMRectList);
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const render = (quota: QuotaSnapshot | null, providerId = "codex") => act(async () => root.render(<><ContextUsageRing quota={quota} providerId={providerId} /><Tooltips /></>));
const hover = () => act(async () => { container.querySelector("button")!.dispatchEvent(new MouseEvent("pointerover", { bubbles: true })); vi.advanceTimersByTime(500); });

it("uses context occupancy, never cumulative spend or subscription remaining", async () => {
  await render({ ...base, windows: [{ id: "weekly", windowMinutes: 10080, usedPercent: 3 }], sessionUsage: { contextUsedTokens: 61000, contextWindowTokens: 100000, totalTokens: 657000000, cacheReadTokens: 640500000, cacheHitRatePercent: 99.2, totalsScope: "session", observedAtUnixMs: 1000 } });
  expect(container.querySelector("button")!.getAttribute("aria-label")).toBe("上下文已用 61%");
  await hover();
  const tooltip = document.querySelector('[role="tooltip"]')!;
  expect(tooltip.textContent).toContain("61,000"); expect(tooltip.textContent).toContain("100,000"); expect(tooltip.textContent).toContain("657,000,000");
  expect(tooltip.textContent).toContain("99.2%"); expect(tooltip.textContent).toContain("会话 · 总 tokens");
});
it("renders percentage-only native usage without inventing token counts", async () => {
  await render({ ...base, sessionUsage: { contextUsedPercent: 42, credits: 0.1, observedAtUnixMs: 1 } }, "kiro");
  await hover(); const tooltip = document.querySelector('[role="tooltip"]')!;
  expect(tooltip.textContent).toContain("42%"); expect(tooltip.textContent).not.toContain("窗口容量"); expect(tooltip.textContent).not.toContain("已用 tokens");
});
it("clears the open panel on session changes and supports keyboard dismissal", async () => {
  await render({ ...base, context: { usedTokens: 1200, windowTokens: 2000 } }); await hover();
  expect(document.querySelector('[role="tooltip"]')!.textContent).toContain("1,200");
  await render(null);
  expect(container.querySelector("button")).toBeNull();
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
  await render({ ...base, sessionUsage: { inputTokens: 81239, observedAtUnixMs: 1 } });
  expect(container.querySelector("button")!.getAttribute("aria-label")).toBe("会话用量（上下文占用未知）");
  await render({ ...base, context: { usedTokens: 0, windowTokens: 2000 } });
  expect(container.querySelector("button")!.getAttribute("aria-label")).toBe("上下文已用 0%");
  await hover();
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
  const button = container.querySelector("button")!;
  vi.spyOn(button, "matches").mockImplementation(selector => selector === ":focus-visible");
  await act(async () => { button.focus(); vi.advanceTimersByTime(1); });
  expect(document.querySelector('[role="tooltip"]')).not.toBeNull();
  expect(button.hasAttribute("aria-describedby")).toBe(true);
});
it("keeps unknown distinct from zero after compaction and handles over-capacity", () => {
  expect(contextUsage({ ...base, context: { usedTokens: 100, windowTokens: 200 }, sessionUsage: { contextUsedTokens: null, contextWindowTokens: 1000, observedAtUnixMs: 1 } }).percent).toBeNull();
  expect(contextUsage({ ...base, sessionUsage: { contextUsedTokens: 0, contextWindowTokens: 1000, observedAtUnixMs: 1 } }).percent).toBe(0);
  expect(contextUsage({ ...base, context: { usedTokens: 1100, windowTokens: 1000 } }).percent).toBeCloseTo(110);
  expect(contextUsage({ ...base, context: { usedTokens: 100, windowTokens: 0 } }).percent).toBeNull();
});
it("labels ZCode native token cache and turn counters", async () => {
  await render({ ...base, context: { usedTokens: 50, windowTokens: 100, cacheHitRatePercent: 80 } }, "zcode"); await hover();
  expect(document.querySelector('[role="tooltip"]')!.textContent).toContain("最新 token 缓存命中");
  await render({ ...base, sessionUsage: { contextUsedPercent: 1, inputTokens: 0, outputTokens: 12, totalsScope: "turn", observedAtUnixMs: 1 } }, "claude-code");
  expect(document.querySelector('[role="tooltip"]')!.textContent).toContain("本轮 · 输入0");
  expect(document.querySelector('[role="tooltip"]')!.textContent).not.toContain("最新 token 缓存命中");
});

it("shows cost-only and known-zero usage without inventing context", async () => {
  for (const usage of [{ costUsd: 0.2 }, { inputTokens: 0 }, { credits: 0 }, { cacheHitRatePercent: 0 }]) {
    await render({ ...base, sessionUsage: { ...usage, observedAtUnixMs: 1 } });
    const button = container.querySelector("button")!;
    expect(button.getAttribute("aria-label")).toBe("会话用量（上下文占用未知）");
    expect(button.textContent).toBe("用量");
    expect(button.querySelector("[stroke-dasharray]")).toBeNull();
    expect(JSON.parse(button.dataset.tooltipRows!)).toContainEqual(["上下文已用", "未知"]);
  }
});
it("hides empty telemetry and never revives stale cache or context", async () => {
  await render({ ...base, context: { usedTokens: 50, windowTokens: 100, cacheHitRatePercent: 99 }, sessionUsage: { observedAtUnixMs: 1 } }, "zcode");
  expect(container.querySelector("button")).toBeNull();
  await render({ ...base, sessionUsage: { costUsd: -1, inputTokens: NaN, cacheHitRatePercent: 101, observedAtUnixMs: 1 } });
  expect(container.querySelector("button")).toBeNull();
});

it("shows observed request totals without merging native counters or filling missing tokens", async () => {
  const report = { truncated: false, records: [
    { requestId: "a", source: "test", inputTokens: 100, outputTokens: 10 },
    { requestId: "b", source: "test", inputTokens: 200, outputTokens: null },
  ] };
  await render({ ...base, sessionUsage: { inputTokens: 9000, requestUsage: report, observedAtUnixMs: 1 } });
  await hover();
  const text = document.querySelector('[role="tooltip"]')!.textContent!;
  expect(text).toContain("已记录模型请求2 次");
  expect(text).toContain("请求 · 输入（含缓存）300");
  expect(text).not.toContain("请求 · 输出");
  expect(text).toContain("不代表完整会话");
  expect(text).toContain("会话 · 输入9,000");
});
