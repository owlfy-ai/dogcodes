import { createContext, Fragment } from "react";
import type { QuotaSnapshotView, QuotaWindow } from "@dogcodes/protocol";
import { getGlobalResolvedLanguage, translate as t } from "./i18n";
import { formatProviderBalance, formatResetTime, pickerQuotaWindows, pickerWindowLabel, quotaColor, quotaNativeResetLabel, quotaWindowHasReset, remainingPercent } from "./quotaPresentation";

// Shared live readings also reach cached conversation pages through React context.
export const AgentQuotaContext = createContext<QuotaSnapshotView[]>([]);

function percentText(remaining: number): string {
  if (remaining > 0 && remaining < 1) return "<1%";
  return `${Math.round(remaining * 10) / 10}%`;
}

function meterReading(window: QuotaWindow) {
  const pendingReset = quotaWindowHasReset(window);
  const remaining = pendingReset ? null : remainingPercent(window.usedPercent);
  const text = remaining === null ? "?" : remaining === 0 ? t("quota.meterUsedUp") : percentText(remaining);
  const detail = remaining === 0 ? t("agentUi.usedUp") : text;
  return { pendingReset, remaining, text, detail };
}

/** Compact quota rows inside an Agent picker option. */
export function AgentQuotaRing({ quota, providerId, model = null }: { quota: QuotaSnapshotView["quota"]; providerId: string; model?: string | null }) {
  const lang = getGlobalResolvedLanguage();
  const windows = pickerQuotaWindows(quota?.windows ?? [], providerId, model);
  if (!windows.length && quota?.balances?.length) {
    const rows = quota.balances.map((balance) => ({
      label: balance.label,
      value: formatProviderBalance(balance.amount, balance.currency, lang),
    }));
    const label = rows.map((row) => row.value).join("\n");
    return <span className="agent-option-quota" data-tooltip={label} aria-label={label}>
      {rows.map((row, index) => <span className="picker-quota-amount" key={`${row.value}:${index}`}>{row.value}</span>)}
    </span>;
  }
  if (!windows.length) {
    const label = t("quota.noReading");
    return <span className="agent-option-quota" data-tooltip={label} aria-label={label}><span className="quota-ring-value picker-quota-amount">?</span></span>;
  }
  const rows = windows.map((window) => {
    const reading = meterReading(window);
    const label = pickerWindowLabel(window, providerId, lang);
    const resetLabel = quota
      ? window.resetsAtUnix != null
        ? t("agentUi.resetAt", { time: formatResetTime(window.resetsAtUnix, Date.now(), lang) })
        : quotaNativeResetLabel(quota, window, lang)
      : "";
    return { id: window.id, label, resetLabel, ...reading };
  });
  const tooltip = rows.map((row) => [ `${row.label} ${row.detail}`, row.resetLabel, row.pendingReset ? t("quota.resetWaiting") : "" ].filter(Boolean).join("\n")).join("\n");
  return (
    <span className="agent-option-quota" data-tooltip={tooltip} aria-label={tooltip}>
      {rows.map((row) => {
        const color = row.remaining === null ? "currentColor" : quotaColor(row.remaining);
        return <Fragment key={row.id}>
          <span className="picker-quota-label">{row.label}</span>
          <span className="picker-quota-bar" aria-hidden="true"><span style={{ width: `${row.remaining ?? 0}%`, background: color }} /></span>
          <span className="quota-ring-value picker-quota-value" style={{ color }}>{row.text}</span>
        </Fragment>;
      })}
    </span>
  );
}
