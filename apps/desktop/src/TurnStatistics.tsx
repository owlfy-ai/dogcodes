import type { TimelineItem, TurnUsage } from "@dogcodes/protocol";
import { providerLabel } from "./agentPresentation";
import { getGlobalResolvedLanguage, translate as t } from "./i18n";

const valid = (value: number | null | undefined): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0;
const number = (value: number) => value.toLocaleString("en-US", { maximumFractionDigits: 0 });
export function turnStatisticsRows(items: TimelineItem[], turnId: string, startedAt: string, completedAt: string): [string, string][] {
  const turn = items.filter(item => item.turnId === turnId);
  const firstOutputMs = turn.find(item => item.turnCompletion?.firstOutputMs != null)?.turnCompletion?.firstOutputMs;
  const usage = turn.find(item => item.turnCompletion?.usage)?.turnCompletion?.usage;
  const rows: [string, string][] = [];
  const provider = turn.find(item => item.providerId)?.providerId;
  if (provider) rows.push(["Agent", providerLabel(provider)]);
  const duration = Date.parse(completedAt) - Date.parse(startedAt);
  const locale = getGlobalResolvedLanguage();
  if (valid(duration)) rows.push([t("turnStats.duration"), t("turnStats.seconds", { value: (duration / 1000).toLocaleString(locale, { maximumFractionDigits: 1 }) })]);
  if (valid(firstOutputMs)) rows.push([t("turnStats.firstOutput"), t("turnStats.seconds", { value: (firstOutputMs / 1000).toLocaleString(locale, { maximumFractionDigits: 2 }) })]);
  if (usage && valid(usage.outputTokensPerSecond)) {
    rows.push([t("turnStats.speed"), t("turnStats.tokensPerSecond", { value: usage.outputTokensPerSecond.toLocaleString(locale, { maximumFractionDigits: 1 }) })]);
  }
  if (usage?.models.length) rows.push([t("modelPicker.model"), [...new Set(usage.models)].join(t("submissionRecovery.nameSeparator"))]);
  if (!usage || usage.scope === "unknown") return rows;
  const requests = usage.scope === "observed_requests";
  if (valid(usage.requestCount)) rows.push([t("turnStats.requests"), t("contextUsage.requestCount", { count: number(usage.requestCount) })]);
  const counters: [keyof TurnUsage, string][] = [
    ["inputTokens", t(requests ? "turnStats.inputWithCache" : "turnStats.input")],
    ["outputTokens", t(requests ? "turnStats.outputWithReasoning" : "turnStats.output")],
    ["cacheReadTokens", t("turnStats.cacheRead")], ["cacheWriteTokens", t("turnStats.cacheWrite")], ["reasoningTokens", t("turnStats.reasoning")],
  ];
  for (const [key, label] of counters) {
    const value = usage[key];
    if (typeof value === "number" && valid(value)) rows.push([label, number(value)]);
  }
  if (requests && valid(usage.inputTokens) && valid(usage.outputTokens) && Number.isSafeInteger(usage.inputTokens + usage.outputTokens)) rows.push([t("turnStats.total"), number(usage.inputTokens + usage.outputTokens)]);
  if (valid(usage.costUsd)) rows.push([t("turnStats.cost"), `$${usage.costUsd.toLocaleString("en-US", { maximumFractionDigits: 6 })}`]);
  if (valid(usage.credits)) rows.push(["Credits", usage.credits.toLocaleString("en-US", { maximumFractionDigits: 4 })]);
  return rows;
}
export function TurnStatistics(props: { items: TimelineItem[]; turnId: string; startedAt: string; completedAt: string }) {
  const rows = turnStatisticsRows(props.items, props.turnId, props.startedAt, props.completedAt);
  return <button type="button" className="turn-action turn-statistics" aria-label={t("turnStats.title")} data-tooltip={t("turnStats.title")} data-tooltip-rows={JSON.stringify(rows)}>
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M4 20h16M7 16v-5m5 5V4m5 12V8"/>
    </svg>
  </button>;
}
