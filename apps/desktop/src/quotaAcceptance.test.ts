import { describe, expect, it } from "vitest";
import type { QuotaSnapshot } from "@dogcodes/protocol";
import { antigravityQuotaForModel, quotaIsExhausted, quotaWindowHasReset, remainingPercent } from "./quotaPresentation";

const now = 1_800_000_000_000;
const snapshot = (usedPercent: number | null): QuotaSnapshot => ({
  windows: [{ id: "5h", usedPercent, windowMinutes: 300, resetsAtUnix: now / 1000 + 30 }],
  collectedAtUnixMs: now - 1000, staleAfterUnixMs: now + 60_000,
  source: "native_pull", confidence: "provider_reported", parserVersion: "acceptance",
});

describe("quota abnormal-state acceptance (fixed clock; no external requests)", () => {
  it.each([0, 20, 99, 99.96, 99.999])("does not block a positive balance at %s%% used", used => {
    expect(quotaIsExhausted(snapshot(used), now)).toBe(false);
    expect(remainingPercent(used)).toBeGreaterThan(0);
  });
  it.each([100, 101])("blocks a fresh exhausted account at %s%% used", used => {
    expect(quotaIsExhausted(snapshot(used), now)).toBe(true);
  });
  it("unblocks at reset without inventing a new balance", () => {
    const quota = snapshot(100);
    expect(quotaIsExhausted(quota, now + 29_999)).toBe(true);
    expect(quotaIsExhausted(quota, now + 30_000)).toBe(false);
    expect(quotaWindowHasReset(quota.windows[0], now + 30_000)).toBe(true);
    expect(quota.windows[0].usedPercent).toBe(100);
  });
  it("keeps a failed refresh's last observation subject to its original TTL", () => {
    const quota = snapshot(100);
    quota.windows[0].resetsAtUnix = null;
    expect(quotaIsExhausted(quota, now)).toBe(true);
    expect(quotaIsExhausted(quota, quota.staleAfterUnixMs)).toBe(false);
    expect(quotaIsExhausted(null, now)).toBe(false);
    expect(quotaIsExhausted(snapshot(null), now)).toBe(false);
    expect(quotaIsExhausted(snapshot(Number.NaN), now)).toBe(false);
    expect(quotaIsExhausted(snapshot(Infinity), now)).toBe(false);
  });
  it("a weekly exhaustion still blocks when the five-hour window resets", () => {
    const quota = snapshot(100);
    quota.windows.push({ id: "week", windowMinutes: 10080, usedPercent: 100 });
    expect(quotaIsExhausted(quota, now + 30_000)).toBe(true);
  });
  it("switches model groups without mutating the shared provider reading", () => {
    const quota = snapshot(100);
    quota.windows[0].scope = "Gemini Models";
    quota.windows.push({ id: "claude", scope: "Claude and GPT models", windowMinutes: 300, usedPercent: 10 });
    expect(quotaIsExhausted(antigravityQuotaForModel(quota, "gemini-3.1-pro-low"), now)).toBe(true);
    expect(quotaIsExhausted(antigravityQuotaForModel(quota, "claude-sonnet-4-6"), now)).toBe(false);
    expect(quotaIsExhausted(antigravityQuotaForModel(quota, "unknown-model"), now)).toBe(false);
    expect(quota.windows.every(w => w.scope)).toBe(true);
  });
  it.each(["套餐内（终端读数）", "工具调用", "MCP"])("never blocks general chat on informational scope %s", scope => {
    const quota = snapshot(100);
    quota.windows[0].scope = scope;
    expect(quotaIsExhausted(quota, now)).toBe(false);
  });
});
