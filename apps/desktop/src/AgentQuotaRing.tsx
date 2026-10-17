import { createContext } from "react";
import type { QuotaSnapshotView } from "@dogcodes/protocol";
import { ringQuotaWindow, quotaWindowHasReset, remainingPercent, quotaIsExhausted, quotaColor, quotaWindowLabel, formatResetTime, quotaNativeResetLabel } from "./quotaPresentation";

// Shared live readings also reach cached conversation pages through React context.
export const AgentQuotaContext = createContext<QuotaSnapshotView[]>([]);

/** Compact, non-interactive quota indicator used inside Agent picker rows. */
export function AgentQuotaRing({ quota, providerId }: { quota: QuotaSnapshotView["quota"]; providerId: string }) {
  const primary = ringQuotaWindow(quota?.windows ?? [], providerId);
  const primaryReset = primary ? quotaWindowHasReset(primary) : false;
  const remaining = primaryReset ? null : remainingPercent(primary?.usedPercent);
  const exhausted = providerId === "antigravity" ? remaining === 0 : quotaIsExhausted(quota);
  const color = exhausted ? "#d93025" : remaining === null ? "currentColor" : quotaColor(remaining);
  const radius = 7;
  const circumference = 2 * Math.PI * radius;
  const dash = ((remaining ?? 0) / 100) * circumference;
  const label = exhausted
    ? "额度已用尽"
    : remaining === null
      ? "额度暂无读数"
      : `${providerId === "cursor" && primary?.scope ? `${primary.scope} · ` : ""}${quotaWindowLabel(primary!.windowMinutes, primary!.id)}剩余额度 ${remaining}%`;
  const resetLabel = primary && quota
    ? primary.resetsAtUnix != null
      ? `重置时间：${formatResetTime(primary.resetsAtUnix)}`
      : quotaNativeResetLabel(quota, primary) || "暂未提供重置时间"
    : "暂未提供重置时间";
  const tooltip = [label, resetLabel, primaryReset ? "已到重置时间，等待读取新额度" : ""].filter(Boolean).join("\n");
  return (
    <span className="agent-option-quota" data-tooltip={tooltip} aria-label={tooltip}>
      <svg width="18" height="18" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="10" cy="10" r={radius} fill="none" stroke="rgba(128,128,128,0.3)" strokeWidth="2.5" />
        <circle
          cx="10" cy="10" r={radius} fill="none"
          stroke={color} strokeWidth="2.5" strokeLinecap="round"
          strokeDasharray={`${dash} ${circumference}`}
          transform="rotate(-90 10 10)"
        />
      </svg>
      <span className="quota-ring-value" aria-hidden="true">
        {exhausted ? "!" : remaining === null ? "?" : `${remaining > 0 && remaining < 1 ? "<1" : Math.round(remaining)}%`}
      </span>
    </span>
  );
}
