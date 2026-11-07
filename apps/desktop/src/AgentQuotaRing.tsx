import { createContext, Fragment, useEffect, useState } from "react";
import type { QuotaSnapshotView, QuotaWindow } from "@dogcodes/protocol";
import { getGlobalResolvedLanguage, translate as t } from "./i18n";
import { composerQuotaWindows, formatProviderBalance, formatResetCountdown, formatResetTime, nativeResetDisplay, pickerQuotaWindows, pickerWindowLabel, quotaColor, quotaIsLow, quotaNativeResetLabel, quotaWindowHasReset, remainingPercent } from "./quotaPresentation";

// Shared live readings also reach cached conversation pages through React context.
export const AgentQuotaContext = createContext<QuotaSnapshotView[]>([]);

const COUNTDOWN_REFRESH_MS = 30_000;

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

/** Compact quota rows inside an Agent picker option, or the shorter set under the composer. */
export function AgentQuotaRing({ quota, providerId, model = null, composer = false }: { quota: QuotaSnapshotView["quota"]; providerId: string; model?: string | null; composer?: boolean }) {
  const lang = getGlobalResolvedLanguage();
  // The countdown is in minutes and a menu can stay open across several of them.
  const [, tick] = useState(0);
  useEffect(() => {
    if (composer) return;
    const timer = setInterval(() => tick(value => value + 1), COUNTDOWN_REFRESH_MS);
    return () => clearInterval(timer);
  }, [composer]);
  const windows = (composer ? composerQuotaWindows : pickerQuotaWindows)(quota?.windows ?? [], providerId, model);
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
    // A native reading has only a date, which a countdown would make look more exact than it is.
    const counting = window.resetsAtUnix != null && !reading.pendingReset;
    const resetText = reading.pendingReset ? t("quota.resetDue")
      : counting ? formatResetCountdown(window.resetsAtUnix!, Date.now(), lang)
      : quota ? nativeResetDisplay(quota, window, lang) : "";
    const exact = counting ? `${t("quota.resetsIn", { time: formatResetCountdown(window.resetsAtUnix!, Date.now(), lang, true) })}\n` : "";
    return { id: window.id, label, resetLabel: `${exact}${resetLabel}`, resetText, ...reading };
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
          {!composer && <span className="picker-quota-reset" data-low={row.remaining !== null && quotaIsLow(row.remaining) || undefined}>{row.resetText}</span>}
        </Fragment>;
      })}
    </span>
  );
}
