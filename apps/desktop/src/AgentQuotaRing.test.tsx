// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { QuotaSnapshotView } from "@dogcodes/protocol";
import { AgentPickerMenu } from "./AgentPickerMenu";
import { AgentQuotaRing } from "./AgentQuotaRing";
import { formatResetCountdown } from "./quotaPresentation";

const NOW = Date.parse("2026-11-07T12:00:00Z");
const inSeconds = (seconds: number) => NOW / 1000 + seconds;
const quota = (windows: unknown[], extra: Record<string, unknown> = {}) => ({ windows, ...extra, confidence: "provider_reported", staleAfterUnixMs: NOW + 1e9 }) as unknown as QuotaSnapshotView["quota"];
let root: Root, host: HTMLDivElement;
beforeEach(() => {
  vi.useFakeTimers({ now: NOW });
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });
const cells = () => [...host.querySelectorAll(".picker-quota-reset")].map(cell => cell.textContent);

it("keeps a countdown to the two units that matter, for each language", () => {
  const time = (seconds: number, lang: Parameters<typeof formatResetCountdown>[2] = "zh-CN") => formatResetCountdown(inSeconds(seconds), NOW, lang);
  expect(time(3 * 86_400 + 12 * 3_600 + 24 * 60)).toBe("3天12小时");
  expect(time(26 * 86_400 + 16 * 3_600 + 36 * 60)).toBe("26天16小时");
  expect(time(4 * 3_600 + 8 * 60)).toBe("4小时8分");
  expect(time(26 * 60)).toBe("26分");
  expect(time(5 * 3_600)).toBe("5小时");
  expect(time(86_400)).toBe("1天");
  expect(time(3 * 86_400 + 12 * 60)).toBe("3天");
  expect(time(2 * 3_600 + 14 * 60, "en")).toBe("2h 14m");
  expect(time(2 * 3_600 + 14 * 60, "ja")).toBe("2時間14分");
  expect(time(2 * 3_600 + 14 * 60, "ko")).toBe("2시간 14분");
});

it("spells out every unit when asked for the exact time", () => {
  expect(formatResetCountdown(inSeconds(3 * 86_400 + 12 * 3_600 + 24 * 60), NOW, "zh-CN", true)).toBe("3天12小时24分钟");
  expect(formatResetCountdown(inSeconds(4 * 3_600 + 8 * 60), NOW, "zh-CN", true)).toBe("4小时8分钟");
});

it("never reads as nothing left while the reset is still ahead", () => {
  expect(formatResetCountdown(inSeconds(1), NOW, "zh-CN")).toBe("1分");
  expect(formatResetCountdown(inSeconds(61), NOW, "zh-CN")).toBe("2分");
  expect(formatResetCountdown(inSeconds(-10), NOW, "zh-CN")).toBe("1分");
});

it("shows a short time beside each bar with the exact one on hover, and keeps counting", async () => {
  const windows = [
    { id: "primary", windowMinutes: 300, usedPercent: 22, resetsAtUnix: inSeconds(2 * 3_600 + 14 * 60) },
    { id: "secondary", windowMinutes: 10_080, usedPercent: 74, resetsAtUnix: inSeconds(3 * 86_400 + 5 * 3_600 + 12 * 60) },
  ];
  await act(async () => root.render(<AgentQuotaRing quota={quota(windows)} providerId="codex" />));
  expect(cells()).toEqual(["2小时14分", "3天5小时"]);
  expect(host.querySelector(".picker-quota-reset svg")).toBeNull();
  const tooltip = host.querySelector(".agent-option-quota")?.getAttribute("data-tooltip");
  expect(tooltip).toContain("2小时14分钟后重置");
  expect(tooltip).toContain("3天5小时12分钟后重置");
  await act(async () => { await vi.advanceTimersByTimeAsync(2 * 60_000); });
  expect(cells()).toEqual(["2小时12分", "3天5小时"]);
});

it("marks the time only where little is left", async () => {
  const windows = [
    { id: "primary", windowMinutes: 300, usedPercent: 79, resetsAtUnix: inSeconds(600) },
    { id: "secondary", windowMinutes: 10_080, usedPercent: 81, resetsAtUnix: inSeconds(600) },
  ];
  await act(async () => root.render(<AgentQuotaRing quota={quota(windows)} providerId="codex" />));
  const low = [...host.querySelectorAll(".picker-quota-reset")].map(cell => cell.hasAttribute("data-low"));
  expect(low).toEqual([false, true]);
});

it("names the columns once, only when a window is showing", async () => {
  const option = (id: string, windows: unknown[]) => ({ key: id, executor: { providerId: id, installationId: id }, quota: quota(windows) });
  const show = (options: ReturnType<typeof option>[]) => act(async () => root.render(<AgentPickerMenu options={options} selectedKey="" onSelect={() => {}} title="选择" label="选择" />));
  const columns = () => document.querySelector(".agent-picker-columns")?.textContent;
  await show([option("deepseek-harness", [])]);
  expect(columns()).toBeUndefined();
  await show([option("deepseek-harness", []), option("codex", [{ id: "primary", windowMinutes: 300, usedPercent: 22 }])]);
  expect(columns()).toBe("剩余重置");
});

it("says when a reset is due, keeps a date-only reset as a date, and leaves unknown blank", async () => {
  await act(async () => root.render(<AgentQuotaRing providerId="codex" quota={quota([{ id: "primary", windowMinutes: 300, usedPercent: 22, resetsAtUnix: inSeconds(-60) }])} />));
  expect(cells()).toEqual(["等待刷新"]);
  await act(async () => root.render(<AgentQuotaRing providerId="kiro" quota={quota([{ id: "kiro-included", windowMinutes: 43_200, usedPercent: 10 }], { nativeResetOn: "2026-12-01" })} />));
  expect(cells()[0]).toContain("12");
  expect(cells()[0]).not.toContain("天");
  await act(async () => root.render(<AgentQuotaRing providerId="codex" quota={quota([{ id: "primary", windowMinutes: 300, usedPercent: 22 }])} />));
  expect(cells()).toEqual([""]);
});

it("leaves the composer's shorter rows as they were", async () => {
  await act(async () => root.render(<AgentQuotaRing composer providerId="codex" quota={quota([{ id: "primary", windowMinutes: 300, usedPercent: 22, resetsAtUnix: inSeconds(600) }])} />));
  expect(host.querySelector(".picker-quota-bar")).not.toBeNull();
  expect(host.querySelector(".picker-quota-reset")).toBeNull();
});
