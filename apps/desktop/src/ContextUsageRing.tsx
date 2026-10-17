import type { QuotaSnapshot, SessionUsageSnapshot } from "@dogcodes/protocol";
import "./contextUsageRing.css";

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
  if (valid(percent)) rows.push(["上下文已用", percentText(percent)]);
  if (valid(used)) rows.push(["已用 tokens", tokens(used)]);
  if (valid(window) && window > 0) rows.push(["窗口容量", tokens(window)]);
  const cache = quota?.sessionUsage ? usage.cacheHitRatePercent : quota?.context?.cacheHitRatePercent;
  if (valid(cache) && cache <= 100) rows.push(["最新 token 缓存命中", `${cache.toFixed(1)}%`]);
  const scope = usage.totalsScope === "turn" ? "本轮" : usage.totalsScope === "request" ? "最近请求" : usage.totalsScope === "native" ? "原生统计" : usage.totalsScope === "observed" ? "已记录" : usage.totalsScope === "branch" ? "当前分支" : "会话";
  const counters: [keyof SessionUsageSnapshot, string][] = [
    ["inputTokens", "输入"], ["outputTokens", "输出"], ["cacheReadTokens", "缓存读取"],
    ["cacheWriteTokens", "缓存写入"], ["reasoningTokens", "推理"], ["totalTokens", "总 tokens"],
  ];
  for (const [key, label] of counters) {
    const value = usage[key];
    if (typeof value === "number" && valid(value)) rows.push([`${scope} · ${label}`, tokens(value)]);
  }
  if (valid(usage.costUsd)) rows.push([`${scope} · 费用`, `$${usage.costUsd.toLocaleString("en-US", { maximumFractionDigits: 4 })}`]);
  if (valid(usage.sessionCostUsd)) rows.push(["会话 · 费用", `$${usage.sessionCostUsd.toLocaleString("en-US", { maximumFractionDigits: 4 })}`]);
  if (valid(usage.credits)) rows.push([`${scope} · Credits`, usage.credits.toLocaleString("en-US", { maximumFractionDigits: 4 })]);
  const report = usage.requestUsage;
  if (report && report.records.length > 0) {
    rows.push(["已记录模型请求", `${tokens(report.records.length)} 次${report.truncated ? "（部分）" : ""}`]);
    rows.push(["请求统计范围", "已取得的请求，不代表完整会话"]);
    for (const [key, label] of [["inputTokens", "请求 · 输入（含缓存）"], ["outputTokens", "请求 · 输出（含推理）"]] as const) {
      const values = report.records.map(record => record[key]);
      const total = values.reduce<number>((sum, value) => sum + (valid(value) ? value : 0), 0);
      if (values.every(valid) && Number.isSafeInteger(total)) rows.push([label, tokens(total)]);
    }
  } else if (report?.unavailableReason && rows.length > 0) {
    rows.push(["请求明细", "原生数据暂不可用"]);
  }
  if (rows.length === 0) return null;
  if (!valid(percent)) rows.unshift(["上下文已用", "未知"]);
  if (valid(usage.observedAtUnixMs) && usage.observedAtUnixMs > 0) rows.push(["最近上报", new Date(usage.observedAtUnixMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })]);
  const circumference = 2 * Math.PI * 8;
  const fraction = valid(percent) ? Math.min(percent, 100) / 100 : null;
  return <button type="button" className={`context-usage-ring${valid(percent) && percent >= 90 ? " critical" : valid(percent) && percent >= 75 ? " high" : ""}`}
    aria-label={valid(percent) ? `上下文已用 ${percentText(percent)}` : "会话用量（上下文占用未知）"}
    data-tooltip="上下文与用量" data-tooltip-rows={JSON.stringify(rows)}>
    <svg width="20" height="20" viewBox="0 0 22 22" aria-hidden="true">
      <circle className="context-usage-track" cx="11" cy="11" r="8" fill="none" strokeWidth="2.5" />
      {fraction !== null && <circle cx="11" cy="11" r="8" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round"
        strokeDasharray={`${fraction * circumference} ${circumference}`} transform="rotate(-90 11 11)" />}
    </svg>
    <span aria-hidden="true">{valid(percent) ? percentText(percent) : "用量"}</span>
  </button>;
}
