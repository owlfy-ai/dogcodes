import type { TimelineItem, TurnUsage } from "@dogcodes/protocol";
import { providerLabel } from "./agentPresentation";

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
  if (valid(duration)) rows.push(["本轮耗时", `${(duration / 1000).toLocaleString("zh-CN", { maximumFractionDigits: 1 })} 秒`]);
  if (valid(firstOutputMs)) rows.push(["首个输出等待", `${(firstOutputMs / 1000).toLocaleString("zh-CN", { maximumFractionDigits: 2 })} 秒`]);
  if (usage && valid(usage.outputTokensPerSecond)) {
    rows.push(["生成速度", `${usage.outputTokensPerSecond.toLocaleString("zh-CN", { maximumFractionDigits: 1 })} Tokens/秒`]);
  }
  if (usage?.models.length) rows.push(["模型", [...new Set(usage.models)].join("、")]);
  if (!usage || usage.scope === "unknown") return rows;
  const requests = usage.scope === "observed_requests";
  if (valid(usage.requestCount)) rows.push(["模型请求", `${number(usage.requestCount)} 次`]);
  const counters: [keyof TurnUsage, string][] = [
    ["inputTokens", requests ? "输入 Tokens（含缓存）" : "输入 Tokens"],
    ["outputTokens", requests ? "输出 Tokens（含推理）" : "输出 Tokens"],
    ["cacheReadTokens", "缓存读取 Tokens"], ["cacheWriteTokens", "缓存写入 Tokens"], ["reasoningTokens", "推理 Tokens"],
  ];
  for (const [key, label] of counters) {
    const value = usage[key];
    if (typeof value === "number" && valid(value)) rows.push([label, number(value)]);
  }
  if (requests && valid(usage.inputTokens) && valid(usage.outputTokens) && Number.isSafeInteger(usage.inputTokens + usage.outputTokens)) rows.push(["合计 Tokens", number(usage.inputTokens + usage.outputTokens)]);
  if (valid(usage.costUsd)) rows.push(["原生费用", `$${usage.costUsd.toLocaleString("en-US", { maximumFractionDigits: 6 })}`]);
  if (valid(usage.credits)) rows.push(["Credits", usage.credits.toLocaleString("en-US", { maximumFractionDigits: 4 })]);
  return rows;
}
export function TurnStatistics(props: { items: TimelineItem[]; turnId: string; startedAt: string; completedAt: string }) {
  const rows = turnStatisticsRows(props.items, props.turnId, props.startedAt, props.completedAt);
  return <button type="button" className="turn-action turn-statistics" aria-label="本轮数据统计" data-tooltip="本轮数据统计" data-tooltip-rows={JSON.stringify(rows)}>
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="M4 20h16M7 16v-5m5 5V4m5 12V8"/>
    </svg>
  </button>;
}
