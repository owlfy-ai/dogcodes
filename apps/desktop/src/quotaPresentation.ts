import { t, type ResolvedLanguage } from "./i18n";
import type { QuotaSnapshot, QuotaWindow } from "@dogcodes/protocol";

/** Only map explicit native model IDs; never infer an agent's default model. */
export function antigravityQuotaForModel(quota: QuotaSnapshot | null, model: string | null): QuotaSnapshot | null {
  if (!quota || !model) return quota;
  const group = model.startsWith("gemini-") ? "Gemini Models"
    : model.startsWith("claude-") || model.startsWith("gpt-") ? "Claude and GPT models" : null;
  if (!group || !quota.windows.some(window => window.scope === group)) return quota;
  return { ...quota, planType: model, windows: quota.windows.map(window =>
    window.scope === group ? { ...window, scope: null } : window) };
}

export function quotaWindowLabel(minutes: number, windowId?: string, lang: ResolvedLanguage = "zh-CN"): string {
  if (windowId === "opencode-go-monthly") return t("agentUi.monthly", undefined, lang);
  if (hasNativeBillingDate(windowId)) return t("agentUi.monthly", undefined, lang);
  const key = minutes === 300 ? "fiveHours" : minutes === 10_080 ? "weekly" : minutes === 0 ? "billingPeriod" : "minutes";
  return t(`agentUi.${key}`, { count: minutes }, lang);
}

/** Overview rings use a named pool, never a synthetic total across model groups. */
export function ringQuotaWindow(windows: QuotaWindow[], providerId?: string): QuotaWindow | null {
  if (providerId === "opencode") return windows.find(window => window.id === "opencode-go-rolling") ?? primaryQuotaWindow(windows);
  if (providerId === "cursor") return windows.find(window => window.id === "cursor-models") ?? primaryQuotaWindow(windows);
  if (providerId !== "antigravity") return primaryQuotaWindow(windows);
  return windows.find(window => window.windowMinutes === 300
    && (window.scope === "Gemini Models" || window.id.startsWith("Gemini Models:"))) ?? null;
}

export function formatProviderBalance(amount: number, currency: string, lang?: ResolvedLanguage): string {
  if (amount > 0 && amount < 0.000001) return `${currency} <${new Intl.NumberFormat(lang, { maximumFractionDigits: 6 }).format(0.000001)}`;
  return new Intl.NumberFormat(lang, { style: "currency", currency, currencyDisplay: "code", maximumFractionDigits: 6 }).format(amount);
}

export function formatQuotaPercent(percent: number, lang: ResolvedLanguage): string {
  return new Intl.NumberFormat(lang, { style: "percent", maximumFractionDigits: 1 }).format(percent / 100);
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

const NATIVE_MONTHS: Record<string, number> = {
  jan: 0, january: 0, feb: 1, february: 1, mar: 2, march: 2, apr: 3, april: 3,
  may: 4, jun: 5, june: 5, jul: 6, july: 6, aug: 7, august: 7,
  sep: 8, sept: 8, september: 8, oct: 9, october: 9, nov: 10, november: 10, dec: 11, december: 11,
};

// Reset readings from native clients carry no time of day; show the date in
// the operating system's own format. `undefined` hands the choice to Intl.
function nativeResetDisplayText(text: string, lang?: ResolvedLanguage): string {
  const iso = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(text);
  if (iso) {
    const date = new Date(Number(iso[1]), Number(iso[2]) - 1, Number(iso[3]));
    return date.toLocaleDateString(lang, date.getFullYear() !== new Date().getFullYear()
      ? { year: "numeric", month: "numeric", day: "numeric" }
      : { month: "numeric", day: "numeric" });
  }
  const named = /^([A-Za-z]+)\.?\s+(\d{1,2})$/.exec(text);
  const month = named ? NATIVE_MONTHS[named[1].toLowerCase()] : undefined;
  if (named && month != null) {
    // The reading names no year and monthly cadence makes it obvious; never add one.
    return new Date(new Date().getFullYear(), month, Number(named[2]))
      .toLocaleDateString(lang, { month: "numeric", day: "numeric" });
  }
  return text;
}

/** Bare reset date for a window whose native reading keeps it in planType. */
export function nativeResetDisplay(quota: QuotaSnapshot, window: QuotaWindow, lang?: ResolvedLanguage): string {
  if (!hasNativeBillingDate(window.id)) return "";
  const text = quota.planType?.match(NATIVE_RESET_DATE)?.[1]?.trim();
  return text ? nativeResetDisplayText(text, lang) : "";
}

export function quotaPlanLabel(quota: QuotaSnapshot, lang: ResolvedLanguage = "zh-CN"): string | null | undefined {
  const plan = quota.windows.some(window => hasNativeBillingDate(window.id))
    ? quota.planType?.replace(NATIVE_RESET_DATE, "") : quota.planType;
  // Translate only adapter-owned display labels; preserve provider plan names.
  const labels: Record<string, string> = {
    "订阅额度": "subscriptionQuota", "按量付费已开启": "onDemandOn",
    "按量付费关闭": "onDemandOff", "另有赠送额度": "bonusQuota", "另有加购额度": "purchasedQuota",
  };
  return plan == null ? plan : plan.split(" · ").map(part => labels[part] ? t(`agentUi.${labels[part]}`, undefined, lang) : part).join(" · ");
}

export function quotaNativeResetLabel(quota: QuotaSnapshot, window: QuotaWindow, lang: ResolvedLanguage = "zh-CN"): string {
  const display = nativeResetDisplay(quota, window, lang);
  return display ? t("agentUi.resetAt", { time: display }, lang) : "";
}

export function quotaScopeLabel(scope: string | null | undefined): string {
  if (scope === "Gemini Models") return "Gemini";
  if (scope === "Claude and GPT models") return "Claude/GPT";
  if (scope === "套餐内（终端读数）" || scope === "套餐内（Kiro）") return "";
  return scope ?? "";
}

export function formatResetTime(unixSeconds: number, nowUnixMs = Date.now(), lang?: ResolvedLanguage): string {
  // Reset timestamps always carry a time of day, so it is shown; a same-day
  // reset needs no date, and the shape follows the operating system locale.
  const date = new Date(unixSeconds * 1000);
  if (date.toDateString() === new Date(nowUnixMs).toDateString())
    return date.toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" });
  return date.toLocaleString(lang, { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
}
