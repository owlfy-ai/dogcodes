import { t, useI18n, type ResolvedLanguage } from "./i18n";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { useRef, useState } from "react";
import type { InstallationSummary, QuotaSnapshotView } from "@dogcodes/protocol";
import { LoadingStatus } from "./LoadingStatus";
import { localizedAgentError } from "./agentI18n";
import "./agentLogin.css";

type AccountInstallation = Partial<Pick<InstallationSummary, "status" | "version" | "diagnostic">>;
export function agentAccountIssue(installation: AccountInstallation | undefined, view?: QuotaSnapshotView, lang: ResolvedLanguage = "zh-CN"): { label: string; detail: string; loginRequired: boolean } | null {
  if (!installation || installation.version === "demo" || view?.quota?.confidence === "demo") return null;
  for (const code of [view?.diagnostic, installation.diagnostic]) {
    switch (code) {
      case "claude_quota_login_required": return { label: t("agentUi.subscriptionLoginMissing", undefined, lang), detail: t("agentUi.subscriptionLoginHint", undefined, lang), loginRequired: true };
      case "authentication_failed":
      case "quota_login_required": return { label: t("agentUi.loginMissing", undefined, lang), detail: t("agentUi.loginMissingHint", undefined, lang), loginRequired: true };
      case "subscription_required":
      case "quota_subscription_required": return { label: t("agentUi.subscriptionMissing", undefined, lang), detail: t("agentUi.subscriptionMissingHint", undefined, lang), loginRequired: false };
      case "access_denied":
      case "quota_access_denied": return { label: t("agentUi.accessDenied", undefined, lang), detail: t("agentUi.accessDeniedHint", undefined, lang), loginRequired: false };
      case "quota_not_configured":
      case "driver_not_configured": return { label: t("agentUi.notConfigured", undefined, lang), detail: t("agentUi.notConfiguredHint", undefined, lang), loginRequired: false };
    }
  }
  return installation.status === "not_configured"
    ? { label: t("agentUi.notConfigured", undefined, lang), detail: t("agentUi.notConfiguredHint", undefined, lang), loginRequired: false }
    : null;
}
export function needsAgentLogin(installation: AccountInstallation | undefined, view?: QuotaSnapshotView) {
  return agentAccountIssue(installation, view)?.loginRequired ?? false;
}

export function AgentLogin({ installationId, name }: {
  installationId: string; name: string;
}) {
  const { resolvedLanguage: lang } = useI18n();
  const busy = useRef(false);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const open = async () => {
    if (busy.current) return;
    if (!isTauri()) { setError("agentUi.loginDesktopOnly"); return; }
    busy.current = true;
    setOpening(true); setError(null);
    try {
      await invoke("open_agent_login", { installationId });
    } catch (cause) {
      setError(cause);
    } finally {
      busy.current = false;
      setOpening(false);
    }
  };
  return <span className="agent-login">
    <button type="button" className="agent-login-button" onClick={() => void open()} disabled={opening}
      aria-label={t("agentUi.loginAction", { name }, lang)} aria-busy={opening} data-tooltip={error ? localizedAgentError(error, "agentUi.loginFailed", lang) : undefined}>
      {opening ? <LoadingStatus>{t("agentUi.opening", undefined, lang)}</LoadingStatus> : error ? t("agentUi.retry", undefined, lang) : t("agentUi.login", undefined, lang)}
    </button>
    {!!error && <span className="agent-login-feedback" role="alert">{localizedAgentError(error, "agentUi.loginFailed", lang)}</span>}
  </span>;
}
