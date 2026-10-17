// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, TimelineItem, TurnUsage } from "@dogcodes/protocol";
import { TurnActions } from "./TurnActions";
import { TurnStatistics, turnStatisticsRows } from "./TurnStatistics";
import { Tooltips } from "./Tooltips";

const start = "2026-10-09T00:00:00Z", end = "2026-10-09T00:00:12.500Z";
const usage: TurnUsage = { scope: "observed_requests", requestCount: 2, models: ["model-a"], inputTokens: 1200, outputTokens: 300, cacheReadTokens: 800, reasoningTokens: 100, costUsd: 0, partial: false };
const item = (turnId: `trn_${string}`, stats?: TurnUsage): TimelineItem => ({ itemId: turnId, taskId: "tsk_test", turnId, kind: "assistant_message", text: "reply", attachments: [], providerId: "codex", sequence: 1, occurredAt: end, turnCompletion: { startedAt: start, completedAt: end, usage: stats } });
let root: Root, host: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.useFakeTimers();
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([{}] as unknown as DOMRectList);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("puts the statistics icon immediately after copy and opens the shared tooltip on hover and keyboard focus", async () => {
  await act(async () => root.render(<><TurnActions client={{} as CoreClient} items={[item("trn_one", usage)]} turnId="trn_one" startedAt={start} completedAt={end}/><Tooltips /></>));
  const copy = host.querySelector('[aria-label="复制本轮回复"]')!;
  const stats = host.querySelector<HTMLButtonElement>('[aria-label="本轮数据统计"]')!;
  expect(copy.nextElementSibling).toBe(stats);
  await act(async () => { stats.dispatchEvent(new MouseEvent("pointerover", { bubbles: true })); vi.advanceTimersByTime(500); });
  const tooltip = document.querySelector('[role="tooltip"]')!;
  expect(tooltip.textContent).toContain("12.5 秒");
  expect(tooltip.textContent).toContain("1,500");
  expect(tooltip.textContent).toContain("$0");
  expect(stats.getAttribute("aria-describedby")).toBe(tooltip.id);
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
  vi.spyOn(stats, "matches").mockImplementation(selector => selector === ":focus-visible");
  await act(async () => { stats.focus(); vi.advanceTimersByTime(500); });
  expect(document.querySelector('[role="tooltip"]')?.textContent).toContain("模型请求2 次");
});

it("keeps earlier turn statistics fixed when later turns arrive", async () => {
  const first = item("trn_one", usage);
  const render = async (items: TimelineItem[]) => act(async () => root.render(<TurnStatistics items={items} turnId="trn_one" startedAt={start} completedAt={end}/>));
  await render([first]);
  const original = host.querySelector("button")!.getAttribute("data-tooltip-rows");
  await render([first, item("trn_two", { ...usage, inputTokens: 99999 })]);
  expect(host.querySelector("button")!.getAttribute("data-tooltip-rows")).toBe(original);
});

it("does not replace unavailable historical or partial counters with zero or another turn", () => {
  const rows = turnStatisticsRows([item("trn_old"), item("trn_new", usage)], "trn_old", start, end);
  expect(rows).toEqual([["Agent", "Codex"], ["本轮耗时", "12.5 秒"]]);
  expect(rows.some(([label]) => label.includes("Tokens") || label.includes("费用"))).toBe(false);
  const partial = turnStatisticsRows([item("trn_one", { scope: "observed_requests", models: [], partial: true, inputTokens: 10 })], "trn_one", start, end);
  expect(partial.some(([label]) => ["统计范围", "数据完整性", "数据说明", "计速范围"].includes(label))).toBe(false);
  expect(partial.some(([label]) => label === "合计 Tokens" || label === "原生费用")).toBe(false);
});

it("distinguishes native turn summaries and credits from normalized request totals", () => {
  const native = turnStatisticsRows([item("trn_one", { ...usage, scope: "native_turn" })], "trn_one", start, end);
  expect(native.some(([label]) => label === "统计范围")).toBe(false);
  expect(native.some(([label]) => label === "合计 Tokens" || label.includes("含缓存"))).toBe(false);
  const credits = turnStatisticsRows([item("trn_one", { scope: "credits_delta", models: [], credits: 0.25, partial: false })], "trn_one", start, end);
  expect(credits).toContainEqual(["Credits", "0.25"]);
  expect(credits.some(([label]) => label === "原生费用")).toBe(false);
});

it("shows persisted first output and measured generation speed without deriving speed from total duration", () => {
  const value = item("trn_one", { ...usage, completeness: "partial", incompleteReason: "部分请求", outputTokensPerSecond: 42.5, timedRequestCount: 2 });
  value.turnCompletion!.firstOutputMs = 1250;
  const rows = turnStatisticsRows([value], "trn_one", start, end);
  expect(rows).toContainEqual(["首个输出等待", "1.25 秒"]);
  expect(rows).toContainEqual(["生成速度", "42.5 Tokens/秒"]);
  expect(rows.some(([label]) => ["统计范围", "数据完整性", "数据说明", "计速范围"].includes(label))).toBe(false);
  value.turnCompletion!.usage = { scope: "unknown", models: [], completeness: "unknown", partial: false };
  const unknown = turnStatisticsRows([value], "trn_one", start, end);
  expect(unknown.some(([label]) => label === "首个输出等待")).toBe(true);
  expect(unknown.some(([label]) => label.includes("生成速度"))).toBe(false);
});

it("shows a confirmed model without adding unavailable usage or explanation rows", () => {
  const rows = turnStatisticsRows([item("trn_one", { scope: "unknown", models: ["native-model"], partial: false, incompleteReason: "unavailable" })], "trn_one", start, end);
  expect(rows).toEqual([["Agent", "Codex"], ["本轮耗时", "12.5 秒"], ["模型", "native-model"]]);
});
