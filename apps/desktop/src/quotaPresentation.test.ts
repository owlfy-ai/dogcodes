import { describe, expect, it } from "vitest";
import { antigravityQuotaForModel, primaryQuotaWindow, quotaIsExhausted, quotaIsStale, remainingPercent, quotaNativeResetLabel, quotaPlanLabel, quotaWindowLabel, ringQuotaWindow } from "./quotaPresentation";

describe("quota presentation", () => {
  it("shows Kiro's included allowance without blocking bonus or paid usage", () => {
    const q = { windows: [{ id: "kiro-included", scope: "套餐内（Kiro）", windowMinutes: 0, usedPercent: 100, resetsAtUnix: null }],
      planType: "KIRO FREE · 重置 2026-10-01（原生日期） · 按量付费已开启", collectedAtUnixMs: 1000, staleAfterUnixMs: 2000,
      source: "native_pull", confidence: "provider_reported", parserVersion: "kiro-account-usage-v1" };
    expect(primaryQuotaWindow(q.windows)?.id).toBe("kiro-included");
    expect(quotaIsExhausted(q, 1500)).toBe(false);
    expect(quotaNativeResetLabel(q, q.windows[0])).toBe("重置时间：2026-10-01");
    expect(quotaPlanLabel(q)).toBe("KIRO FREE · 按量付费已开启");
  });
  it("maps only the explicitly selected model group", () => {
    const quota = { windows: [
      { id: "gemini", scope: "Gemini Models", windowMinutes: 300, usedPercent: 100 },
      { id: "claude", scope: "Claude and GPT models", windowMinutes: 300, usedPercent: 20 },
    ], collectedAtUnixMs: 1000, staleAfterUnixMs: 2000, source: "native_pull", confidence: "provider_reported", parserVersion: "test" };
    const claude = antigravityQuotaForModel(quota, "claude-sonnet-4-6")!;
    expect(primaryQuotaWindow(claude.windows)?.usedPercent).toBe(20);
    expect(quotaIsExhausted(claude, 1500)).toBe(false);
    expect(quotaIsExhausted(antigravityQuotaForModel(quota, "gemini-3.1-pro-high"), 1500)).toBe(true);
    expect(primaryQuotaWindow(antigravityQuotaForModel(quota, null)!.windows)).toBeNull();
    expect(quota.windows[0].scope).toBe("Gemini Models");
  });
  it("keeps Cursor model pools separate and never blocks spillover or paid usage", () => {
    const q = { windows: [
      { id: "cursor-models", scope: "Cursor Models", windowMinutes: 0, usedPercent: 20 },
      { id: "cursor-other-models", scope: "Other Models", windowMinutes: 0, usedPercent: 100 },
    ], planType: "Pro · 重置 Oct 4（原生日期）· 按量付费关闭", collectedAtUnixMs: 1000, staleAfterUnixMs: 2000,
    source: "native_tui", confidence: "provider_reported", parserVersion: "cursor-tui-usage-v2" };
    expect(primaryQuotaWindow(q.windows)).toBeNull();
    expect(ringQuotaWindow([...q.windows].reverse(), "cursor")?.id).toBe("cursor-models");
    expect(quotaIsExhausted(q, 1500)).toBe(false);
    expect(quotaIsExhausted({ ...q, windows: q.windows.map(w => ({ ...w, usedPercent: 100 })) }, 1500)).toBe(false);
    for (const window of q.windows) {
      expect(quotaWindowLabel(window.windowMinutes, window.id)).toBe("每月");
      expect(quotaNativeResetLabel(q, window)).toBe("重置时间：10/4");
    }
    expect(quotaPlanLabel(q)).toBe("Pro· 按量付费关闭");
  });
  it("displays Cursor's informational account pool without blocking", () => {
    const windows = [{ id: "included", scope: "套餐内（终端读数）", windowMinutes: 0, usedPercent: 100 }];
    expect(primaryQuotaWindow(windows)?.id).toBe("included");
    expect(quotaIsExhausted({ windows, collectedAtUnixMs: 1000, staleAfterUnixMs: 2000,
      source: "native_tui", confidence: "provider_reported", parserVersion: "test" }, 1500)).toBe(false);
  });
  it("does not select or block on an unresolved model pool", () => {
    const windows = [{ id: "gemini", scope: "Gemini Models", windowMinutes: 300, usedPercent: 100 }];
    expect(primaryQuotaWindow(windows)).toBeNull();
    expect(quotaIsExhausted({ windows, collectedAtUnixMs: 1000, staleAfterUnixMs: 2000,
      source: "native_pull", confidence: "provider_reported", parserVersion: "test" }, 1500)).toBe(false);
  });
  it("prefers the five-hour account window even when input order differs", () => {
    const primary = primaryQuotaWindow([
      { id: "secondary", windowMinutes: 10_080, usedPercent: 63 },
      { id: "primary", windowMinutes: 300, usedPercent: 39 },
    ]);
    expect(primary?.id).toBe("primary");
  });

  it("falls back to the shortest provider-reported window", () => {
    const primary = primaryQuotaWindow([
      { id: "long", windowMinutes: 1_440, usedPercent: 20 },
      { id: "short", windowMinutes: 60, usedPercent: 80 },
    ]);
    expect(primary?.id).toBe("short");
  });

  it("clamps remaining quota and marks TTL expiry", () => {
    expect(remainingPercent(null)).toBeNull();
    expect(remainingPercent(Number.NaN)).toBeNull();
    expect(remainingPercent(39)).toBe(61);
    expect(remainingPercent(110)).toBe(0);
    expect(quotaIsStale(1_000, 1_000)).toBe(true);
  });

  it("only treats a fresh zero-percent primary window as exhausted", () => {
    const quota = {
      windows: [{ id: "primary", windowMinutes: 300, usedPercent: 100 }],
      collectedAtUnixMs: 1_000,
      staleAfterUnixMs: 2_000,
      source: "native_pull",
      confidence: "provider_reported",
      parserVersion: "test",
    };
    expect(quotaIsExhausted(quota, 1_500)).toBe(true);
    expect(quotaIsExhausted(quota, 2_000)).toBe(false);
    expect(quotaIsExhausted({ ...quota, windows: [] }, 1_500)).toBe(false);
    expect(quotaIsExhausted({
      ...quota,
      windows: [{ ...quota.windows[0], resetsAtUnix: 1 }],
    }, 1_500)).toBe(false);
    expect(quotaIsExhausted({
      ...quota,
      windows: [
        { id: "five-hour", windowMinutes: 300, usedPercent: 20 },
        { id: "weekly", windowMinutes: 10_080, usedPercent: 100 },
      ],
    }, 1_500)).toBe(true);
    expect(quotaIsExhausted({ ...quota, confidence: "demo" }, 1_500)).toBe(false);
  });
});
