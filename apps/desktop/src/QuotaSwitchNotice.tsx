import { useRef } from "react";
import type { ExecutorRef, QuotaSnapshot } from "@dogcodes/protocol";
import { providerLabel } from "./agentPresentation";
import { focusComposerInput } from "./composerFocus";
import { translate as t, type ResolvedLanguage } from "./i18n";
import { formatResetTime, quotaIsExhausted, quotaIsStale, quotaWindowHasReset, remainingPercent } from "./quotaPresentation";
import "./quotaSwitchNotice.css";

interface Option { executor: ExecutorRef; quota: QuotaSnapshot | null }

/** A failed turn says the quota ran out, unless the agent has reported otherwise since. */
export function quotaLooksExhausted(quota: QuotaSnapshot | null | undefined, turnExhausted: boolean): boolean {
  const reportedSince = Boolean(quota && quota.confidence === "provider_reported" && !quotaIsStale(quota.staleAfterUnixMs)
    && quota.windows.some(window => !window.scope && !quotaWindowHasReset(window) && remainingPercent(window.usedPercent) !== null));
  return quotaIsExhausted(quota) || (turnExhausted && !reportedSince);
}

/** Options already follow the person's own agent order, so the first usable one is theirs. */
export function quotaSwitchTarget(current: ExecutorRef, options: Option[]): ExecutorRef | undefined {
  return options.find(option => !quotaIsExhausted(option.quota)
    && !(option.executor.providerId === current.providerId && option.executor.installationId === current.installationId))?.executor;
}

/** When sending works again: the last of the used-up windows to reset. */
export function quotaResumesAt(quota: QuotaSnapshot | null | undefined): number | undefined {
  const resets = (quota?.windows ?? []).filter(window => !window.scope && !quotaWindowHasReset(window) && window.usedPercent != null && window.usedPercent >= 100)
    .flatMap(window => window.resetsAtUnix == null ? [] : [window.resetsAtUnix]);
  return resets.length ? Math.max(...resets) : undefined;
}

export function QuotaSwitchNotice({ current, quota, turnExhausted, options, running, lang, onSwitch }: {
  current: ExecutorRef; quota: QuotaSnapshot | null | undefined; turnExhausted: boolean; options: Option[]; running: boolean;
  lang?: ResolvedLanguage; onSwitch: (executor: ExecutorRef) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  if (running || !quotaLooksExhausted(quota, turnExhausted)) return null;
  const agent = providerLabel(current.providerId);
  const target = quotaSwitchTarget(current, options);
  const resumes = quotaResumesAt(quota);
  return <div ref={host} className="quota-switch-notice">
    <span role="status">
      {resumes ? t("composer.quotaOutUntil", { agent, time: formatResetTime(resumes, Date.now(), lang) }, lang) : t("composer.quotaOut", { agent }, lang)}
      {!target && ` ${t("composer.quotaNoOtherAgent", undefined, lang)}`}
    </span>
    {target && <button type="button" data-tooltip={t("composer.quotaSwitchTooltip", { agent: providerLabel(target.providerId) }, lang)}
      onClick={() => { focusComposerInput(host.current); onSwitch(target); }}>
      {t("composer.quotaSwitch", { agent: providerLabel(target.providerId) }, lang)}
    </button>}
  </div>;
}
