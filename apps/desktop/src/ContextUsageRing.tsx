import type { QuotaSnapshot, SessionUsageSnapshot } from "@dogcodes/protocol";
import "./contextUsageRing.css";
import { translate as t } from "./i18n";

const valid = (value: number | null | undefined): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0;
const tokens = (value: number) => new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 }).format(value);
const percentText = (value: number) => value > 0 && value < 1 ? "<1%" : `${Math.round(value)}%`;

export function contextUsage(quota: QuotaSnapshot | null) {
  const usage: Partial<SessionUsageSnapshot> = quota?.sessionUsage ?? {};
  // A native reading with an explicitly unknown numerator (e.g. after Pi
  // compaction) must not fall back to an older context snapshot.
  const used = quota?.sessionUsage ? usage.contextUsedTokens : quota?.context?.usedTokens;
  const window = quota?.sessionUsage ? usage.contextWindowTokens : quota?.context?.windowTokens;
  const percent = valid(usage.contextUsedPercent) ? usage.contextUsedPercent
    : valid(used) && valid(window) && window > 0 ? used / window * 100 : null;
  return { usage, used, window, percent };
}

export function ContextUsageRing({ quota }: { quota: QuotaSnapshot | null; providerId: string }) {
  const { usage, used, window, percent } = contextUsage(quota);
  const rows: [string, string][] = [];
  if (valid(percent)) rows.push([t("contextUsage.used"), percentText(percent)]);
  if (valid(used)) rows.push([t("contextUsage.usedTokens"), tokens(used)]);
  if (valid(window) && window > 0) rows.push([t("contextUsage.window"), tokens(window)]);
  const cache = quota?.sessionUsage ? usage.cacheHitRatePercent : quota?.context?.cacheHitRatePercent;
  if (valid(cache) && cache <= 100) rows.push([t("contextUsage.latestCacheHit"), `${cache.toFixed(1)}%`]);
  const scope = t(`contextUsage.scope.${usage.totalsScope === "turn" ? "turn" : usage.totalsScope === "request" ? "request" : usage.totalsScope === "native" ? "native" : usage.totalsScope === "observed" ? "observed" : usage.totalsScope === "branch" ? "branch" : "session"}`);
  const counters: [keyof SessionUsageSnapshot, string][] = [
    ["inputTokens", t("contextUsage.input")], ["outputTokens", t("contextUsage.output")], ["cacheReadTokens", t("contextUsage.cacheRead")],
    ["cacheWriteTokens", t("contextUsage.cacheWrite")], ["reasoningTokens", t("contextUsage.reasoning")], ["totalTokens", t("contextUsage.total")],
  ];
  for (const [key, label] of counters) {
    const value = usage[key];
    if (typeof value === "number" && valid(value)) rows.push([`${scope} · ${label}`, tokens(value)]);
  }
  if (valid(usage.costUsd)) rows.push([`${scope} · ${t("contextUsage.cost")}`, `$${usage.costUsd.toLocaleString("en-US", { maximumFractionDigits: 4 })}`]);
  if (valid(usage.sessionCostUsd)) rows.push([`${t("contextUsage.scope.session")} · ${t("contextUsage.cost")}`, `$${usage.sessionCostUsd.toLocaleString("en-US", { maximumFractionDigits: 4 })}`]);
  if (valid(usage.credits)) rows.push([`${scope} · Credits`, usage.credits.toLocaleString("en-US", { maximumFractionDigits: 4 })]);
  const report = usage.requestUsage;
  if (report && report.records.length > 0) {
    rows.push([t("contextUsage.requests"), t(report.truncated ? "contextUsage.requestCountPartial" : "contextUsage.requestCount", { count: tokens(report.records.length) })]);
    for (const [key, label] of [["inputTokens", t("contextUsage.requestInput")], ["outputTokens", t("contextUsage.requestOutput")]] as const) {
      const values = report.records.map(record => record[key]);
      const total = values.reduce<number>((sum, value) => sum + (valid(value) ? value : 0), 0);
      if (values.every(valid) && Number.isSafeInteger(total)) rows.push([label, tokens(total)]);
    }
  } else if (report?.unavailableReason && rows.length > 0) {
    rows.push([t("contextUsage.requestDetails"), t("contextUsage.nativeUnavailable")]);
  }
  if (rows.length === 0) return null;
  if (!valid(percent)) rows.unshift([t("contextUsage.used"), t("agentUi.unknown")]);
  if (valid(usage.observedAtUnixMs) && usage.observedAtUnixMs > 0) rows.push([t("contextUsage.reported"), new Date(usage.observedAtUnixMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })]);
  const circumference = 2 * Math.PI * 8;
  const fraction = valid(percent) ? Math.min(percent, 100) / 100 : null;
  return <button type="button" className={`context-usage-ring${valid(percent) && percent >= 90 ? " critical" : valid(percent) && percent >= 75 ? " high" : ""}`}
    aria-label={valid(percent) ? t("contextUsage.usedLabel", { percent: percentText(percent) }) : t("contextUsage.unknownLabel")}
    data-tooltip={t("contextUsage.title")} data-tooltip-rows={JSON.stringify(rows)}>
    <svg width="20" height="20" viewBox="0 0 22 22" aria-hidden="true">
      <circle className="context-usage-track" cx="11" cy="11" r="8" fill="none" strokeWidth="2.5" />
      {fraction !== null && <circle cx="11" cy="11" r="8" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round"
        strokeDasharray={`${fraction * circumference} ${circumference}`} transform="rotate(-90 11 11)" />}
    </svg>
    <span aria-hidden="true">{valid(percent) ? percentText(percent) : t("contextUsage.usage")}</span>
  </button>;
}
