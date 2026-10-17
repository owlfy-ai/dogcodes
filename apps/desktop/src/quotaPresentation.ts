import type { QuotaSnapshot, QuotaWindow } from "@dogcodes/protocol";

const QUOTA_WINDOW_LABELS: Record<number, string> = {
  300: "5 小时",
  10_080: "每周",
  0: "当前账期",
};

/** Only map explicit native model IDs; never infer an agent's default model. */
export function antigravityQuotaForModel(quota: QuotaSnapshot | null, model: string | null): QuotaSnapshot | null {
  if (!quota || !model) return quota;
  const group = model.startsWith("gemini-") ? "Gemini Models"
    : model.startsWith("claude-") || model.startsWith("gpt-") ? "Claude and GPT models" : null;
  if (!group || !quota.windows.some(window => window.scope === group)) return quota;
  return { ...quota, planType: model, windows: quota.windows.map(window =>
    window.scope === group ? { ...window, scope: null } : window) };
}

export function quotaWindowLabel(minutes: number, windowId?: string): string {
  if (hasNativeBillingDate(windowId)) return "每月";
  return QUOTA_WINDOW_LABELS[minutes] ?? `${minutes} 分钟`;
}

/** Overview rings use a named pool, never a synthetic total across model groups. */
export function ringQuotaWindow(windows: QuotaWindow[], providerId?: string): QuotaWindow | null {
  if (providerId === "cursor") return windows.find(window => window.id === "cursor-models") ?? primaryQuotaWindow(windows);
  if (providerId !== "antigravity") return primaryQuotaWindow(windows);
  return windows.find(window => window.windowMinutes === 300
    && (window.scope === "Gemini Models" || window.id.startsWith("Gemini Models:"))) ?? null;
}

export function remainingPercent(usedPercent: number | null | undefined): number | null {
  if (usedPercent == null || !Number.isFinite(usedPercent)) return null;
  const remaining = Math.max(0, Math.min(100, 100 - usedPercent));
  // Rounding must not turn a small positive balance into an exhausted bucket.
  return remaining > 0 && remaining < 0.1 ? 0.1 : Math.round(remaining * 10) / 10;
}

export function quotaColor(remaining: number): string {
  return remaining > 50 ? "#34a853" : remaining > 20 ? "#e2b96f" : "#d93025";
}

/** Prefer the account five-hour bucket, then the shortest reported window. */
export function primaryQuotaWindow(windows: QuotaWindow[]): QuotaWindow | null {
  // A single informational account pool can be displayed but never blocks.
  // Multiple model groups still require a verified active-model association.
  const accountWindows = windows.filter((window) => !window.scope);
  windows = accountWindows.length ? accountWindows
    : windows.every((window) => window.scope === "套餐内（终端读数）"
      || (window.id === "kiro-included" && window.scope === "套餐内（Kiro）")) ? windows : [];
  return windows.find((window) => window.windowMinutes === 300)
    ?? [...windows].sort((left, right) => left.windowMinutes - right.windowMinutes)[0]
    ?? null;
}

export function quotaIsStale(staleAfterUnixMs: number, nowUnixMs = Date.now()): boolean {
  return nowUnixMs >= staleAfterUnixMs;
}

export function quotaWindowHasReset(window: QuotaWindow, nowUnixMs = Date.now()): boolean {
  return window.resetsAtUnix != null && window.resetsAtUnix * 1_000 <= nowUnixMs;
}

/** Any fresh, provider-reported active window at 0% can block sending. */
export function quotaIsExhausted(quota: QuotaSnapshot | null | undefined, nowUnixMs = Date.now()): boolean {
  if (!quota || quota.confidence !== "provider_reported" || quotaIsStale(quota.staleAfterUnixMs, nowUnixMs)) return false;
  return quota.windows.some((window) => {
    if (window.scope) return false;
    if (quotaWindowHasReset(window, nowUnixMs)) return false;
    return window.usedPercent != null && Number.isFinite(window.usedPercent) && window.usedPercent >= 100;
  });
}

// Native adapters preserve date-only resets in planType. Keep them
// as display text: it must never drive expiry or quota enforcement.
const NATIVE_RESET_DATE = / · 重置 ([^·]+?)（原生日期）/;
function hasNativeBillingDate(id?: string): boolean {
  return id === "cursor-included" || id === "cursor-models" || id === "cursor-other-models" || id === "kiro-included";
}

export function quotaPlanLabel(quota: QuotaSnapshot): string | null | undefined {
  return quota.windows.some(window => hasNativeBillingDate(window.id))
    ? quota.planType?.replace(NATIVE_RESET_DATE, "") : quota.planType;
}

export function quotaNativeResetLabel(quota: QuotaSnapshot, window: QuotaWindow): string {
  if (!hasNativeBillingDate(window.id)) return "";
  const date = quota.planType?.match(NATIVE_RESET_DATE)?.[1]?.trim();
  if (!date) return "";
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const parts = /^(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) (\d{1,2})$/.exec(date);
  const display = parts ? `${months.indexOf(parts[1]) + 1}/${Number(parts[2])}` : date;
  return `重置时间：${display}`;
}

export function quotaScopeLabel(scope: string | null | undefined): string {
  if (scope === "Gemini Models") return "Gemini";
  if (scope === "Claude and GPT models") return "Claude/GPT";
  if (scope === "套餐内（终端读数）" || scope === "套餐内（Kiro）") return "";
  return scope ?? "";
}

export function formatResetTime(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000);
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  return sameDay
    ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : date.toLocaleDateString([], { month: "numeric", day: "numeric" });
}
