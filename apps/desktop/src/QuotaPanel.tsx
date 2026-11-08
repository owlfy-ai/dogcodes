import { useRef, useState, type DragEvent } from "react";
import type { InstallationSummary, QuotaSnapshotView } from "@dogcodes/protocol";
import { t, useI18n, type ResolvedLanguage } from "./i18n";
import { AgentLogin, agentAccountIssue, needsAgentLogin } from "./AgentLogin";
import { AGENT_DRAG_TYPE, AgentManagementControls } from "./AgentManagementControls";
import { InstallationLink, supportedAgents } from "./AgentWelcome";
import { agentDropSlot, moveAgentToSlot, orderAgents, useAgentPreferences } from "./agentPreferences";
import { AgentIcon, providerLabel } from "./agentPresentation";
import { AgentUpdateControl, type AgentUpdateActions } from "./agentUpdates";
import "./agentManagement.css";
import { quotaShowsLastReading } from "./quotaFallback";
import { localizedAgentMessage } from "./agentI18n";
import type { HostAgentInstall } from "./executionTargetClient";
import { visibleQuotaViews } from "./quotaInstallations";
import { formatProviderBalance, formatQuotaPercent, formatResetTime, quotaColor, quotaNativeResetLabel, quotaPlanLabel, quotaScopeLabel, quotaWindowHasReset, quotaWindowLabel, remainingPercent } from "./quotaPresentation";

export interface HostAgentActions {
  supports: (providerId: string) => boolean;
  installs: HostAgentInstall[];
  install: (providerId: string) => void;
  login: (installationId: string, providerId: string) => void;
}

export function quotaDiagnostic(code?: string | null, lang?: ResolvedLanguage): string {
  switch (code) {
    case "quota_network_error": return t("agentUi.quotaNetworkHint", undefined, lang);
    case "quota_timeout": return t("agentUi.quotaTimeoutHint", undefined, lang);
    case "quota_login_required": return t("agentUi.quotaLoginHint", undefined, lang);
    case "quota_subscription_required": return t("agentUi.quotaSubscriptionHint", undefined, lang);
    case "quota_access_denied": return t("quota.accessDeniedHint", undefined, lang);
    case "quota_not_configured": return t("agentUi.quotaConfigurationHint", undefined, lang);
    case "quota_exhausted": return t("agentUi.quotaExhaustedHint", undefined, lang);
    case "quota_rate_limited": return t("agentUi.quotaRateHint", undefined, lang);
    case "opencode_go_not_configured": return t("agentUi.goConfigurationHint", undefined, lang);
    case "claude_quota_custom_provider": return t("agentUi.claudeCustomHint", undefined, lang);
    case "claude_quota_login_required": return t("agentUi.claudeLoginHint", undefined, lang);
    case "claude_quota_transport_unavailable": return t("agentUi.curlMissing", undefined, lang);
    case "claude_quota_rate_limited": return t("agentUi.claudeRateHint", undefined, lang);
    case "claude_quota_token_expired": return t("quota.claudeTokenExpiredHint", undefined, lang);
    case "claude_quota_network_error":
    case "claude_quota_timeout": return t("agentUi.claudeNetworkHint", undefined, lang);
    case "claude_quota_invalid_response":
    case "claude_quota_no_windows": return t("agentUi.claudeInvalidHint", undefined, lang);
    default: return t("agentUi.quotaEmptyHint", undefined, lang);
  }
}

/// Overall quota panel: every connected agent with a quota source.
export function QuotaPanel({
  views,
  installations = [],
  agentUpdates,
  management = false,
  onDetect,
  host,
}: {
  management?: boolean;
  /** The agents of a server: what it can run and how to act on them over there. */
  host?: HostAgentActions;
  /** Scans for the given agents again, so ones installed after first launch can be enabled. */
  onDetect?: (providerIds: string[]) => Promise<void>;
  views: QuotaSnapshotView[];
  installations?: Array<Pick<InstallationSummary, "installationId" | "version"> & Partial<Pick<InstallationSummary, "providerId" | "status" | "diagnostic">>>;
  agentUpdates?: AgentUpdateActions;
}) {
  const { resolvedLanguage: lang } = useI18n();
  const preferences = useAgentPreferences();
  const [dragged, setDragged] = useState<string | null>(null);
  // The drag source is read synchronously from a ref; the state only styles the card a frame later,
  // so the drag image captured at dragstart is the card at full opacity.
  const draggedRef = useRef<string | null>(null);
  const [drop, setDrop] = useState<{ providerId: string; edge: "before" | "after"; slot: number } | null>(null);
  const [axis, setAxis] = useState<"x" | "y">("y");
  const [announcement, setAnnouncement] = useState("");
  const [detecting, setDetecting] = useState(false);
  const installedViews = management ? installations.filter(item => item.providerId && item.providerId !== "fake").map(item =>
    views.find(view => view.installationId === item.installationId) ?? { providerId: item.providerId!, installationId: item.installationId, quota: null, diagnostic: null }) : visibleQuotaViews(views, installations);
  // Only installed agents take part in ordering; known but missing ones are listed compactly below.
  const uninstalled = management ? supportedAgents.filter(agent => (!host || host.supports(agent.id)) && !installedViews.some(view => view.providerId === agent.id)) : [];
  const visible = orderAgents(installedViews, item => item.providerId, preferences);
  const providerOrder = visible.map(view => view.providerId);
  const endDrag = () => { draggedRef.current = null; setDragged(null); setDrop(null); };
  const announce = (providerId: string, position: number) => setAnnouncement(t("agentUi.movedTo", { name: providerLabel(providerId, lang), position: position + 1, total: providerOrder.length }, lang));
  const startDrag = (event: DragEvent<HTMLButtonElement>, providerId: string) => {
    event.dataTransfer.setData(AGENT_DRAG_TYPE, providerId);
    event.dataTransfer.effectAllowed = "move";
    const card = event.currentTarget.closest<HTMLElement>(".agent-management-card");
    const grid = card?.parentElement;
    if (card && event.dataTransfer.setDragImage) {
      const rect = card.getBoundingClientRect();
      event.dataTransfer.setDragImage(card, event.clientX - rect.left, event.clientY - rect.top);
    }
    // Two columns: cards sit side by side in reading order, so the halves are left/right; one column: top/bottom.
    setAxis(grid && getComputedStyle(grid).gridTemplateColumns.trim().split(/\s+/).filter(Boolean).length > 1 ? "x" : "y");
    draggedRef.current = providerId;
    setAnnouncement("");
    requestAnimationFrame(() => { if (draggedRef.current === providerId) setDragged(providerId); });
  };
  const dragOver = (event: DragEvent<HTMLDivElement>) => {
    const source = draggedRef.current;
    if (!management || !source || !event.dataTransfer.types.includes(AGENT_DRAG_TYPE)) return;
    event.preventDefault();
    const card = (event.target as Element).closest<HTMLElement>(".agent-management-card[data-provider-id]");
    // Gaps between cards keep the last indicator, so it does not flicker while crossing them.
    if (!card) { event.dataTransfer.dropEffect = drop ? "move" : "none"; return; }
    const rect = card.getBoundingClientRect();
    const edge = axis === "x" ? (event.clientX < rect.left + rect.width / 2 ? "before" : "after") : (event.clientY < rect.top + rect.height / 2 ? "before" : "after");
    const providerId = card.dataset.providerId!;
    const slot = agentDropSlot(source, providerId, edge, providerOrder);
    event.dataTransfer.dropEffect = slot == null ? "none" : "move";
    const next = slot == null ? null : { providerId, edge, slot } as const;
    if (next?.providerId !== drop?.providerId || next?.edge !== drop?.edge) setDrop(next);
  };
  const dragLeave = (event: DragEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const inside = event.clientX >= rect.left && event.clientX <= rect.right && event.clientY >= rect.top && event.clientY <= rect.bottom;
    if (!inside) setDrop(null);
  };
  const dropCard = (event: DragEvent<HTMLDivElement>) => {
    const source = draggedRef.current;
    if (!source || event.dataTransfer.getData(AGENT_DRAG_TYPE) !== source) return;
    event.preventDefault();
    // Only the slot the indicator showed is applied; no indicator means the order stays as it is.
    if (drop) { moveAgentToSlot(source, drop.slot, providerOrder); announce(source, drop.slot > providerOrder.indexOf(source) ? drop.slot - 1 : drop.slot); }
    endDrag();
  };
  const versionFor = (installationId: string) => {
    const engine = installations.find((installation) => installation.installationId === installationId)?.version;
    const desktop = agentUpdates?.updates[installationId]?.desktopVersion;
    const provider = visible.find(view => view.installationId === installationId)?.providerId;
    if (desktop) return t("agentUi.clientEngine", { client: desktop, engine: engine ?? t("agentUi.unknownVersion", undefined, lang) }, lang);
    return provider === "codex" || provider === "zcode" ? t("agentUi.engine", { version: engine ?? t("agentUi.unknownVersion", undefined, lang) }, lang) : engine ?? t("agentUi.unknownVersion", undefined, lang);
  };
  const cards = <div className="quota-panel agent-management-grid" data-dragging={management && (dragged || drop) ? axis : undefined}
    onDragOver={management ? dragOver : undefined} onDragLeave={management ? dragLeave : undefined} onDrop={management ? dropCard : undefined}>
    {visible.map(view => {
      const quota = view.quota;
      const installed = installations.some(item => item.installationId === view.installationId);
      const installation = installations.find(item => item.installationId === view.installationId);
      const loginAgent = supportedAgents.find(agent => agent.id === view.providerId);
      const accountIssue = agentAccountIssue(installation, view, lang);
      // A server's only way to sign in is its own terminal, so any missing login gets offered.
      const loginRequired = management && loginAgent && (needsAgentLogin(installation, view) || (!!host && installation?.status === "not_configured"));
      const enabled = !preferences.disabled.includes(view.providerId);
      return <section className="agent-management-card" key={view.installationId} aria-label={providerLabel(view.providerId, lang)}
        data-disabled={management && !enabled || undefined} data-provider-id={management ? view.providerId : undefined}
        data-dragging={management && dragged === view.providerId || undefined} data-drop-edge={drop?.providerId === view.providerId ? drop.edge : undefined}>
        {management && !host && <AgentManagementControls providerId={view.providerId} enabled={enabled} order={providerOrder}
          onDragStart={event => startDrag(event, view.providerId)} onDragEnd={endDrag} onMoved={announce} />}
        <header className="agent-card-heading">
          <AgentIcon providerId={view.providerId} />
          <div className="agent-card-identity"><h3>{providerLabel(view.providerId, lang)}</h3>
            {loginRequired ? <span className="agent-card-login-status">{accountIssue!.label}{host
              ? <button type="button" className="agent-login-button" aria-label={t("agentUi.loginAction", { name: loginAgent.name }, lang)} onClick={() => host.login(view.installationId, view.providerId)}>{t("agentUi.login", undefined, lang)}</button>
              : <AgentLogin installationId={view.installationId} name={loginAgent.name} />}</span>
              : <span>{accountIssue?.label ?? (quota ? quotaPlanLabel(quota, lang) || (quota.balances?.length ? t("agentUi.providerBalance", undefined, lang) : t("agentUi.subscriptionQuota", undefined, lang)) : t("agentUi.quotaUnavailable", undefined, lang))}</span>}
          </div>
          {quota && <time className="agent-card-updated" dateTime={new Date(quota.collectedAtUnixMs).toISOString()} data-tooltip={new Date(quota.collectedAtUnixMs).toLocaleString(lang)}>{t("agentUi.updatedAt", { time: new Date(quota.collectedAtUnixMs).toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" }) }, lang)}</time>}
        </header>
        <div className="agent-card-content">
          {quota ? <>
            {!!quota.balances?.length && <div className="agent-provider-balances" aria-label={t("agentUi.providerBalance", undefined, lang)}>
              {quota.balances.map(balance => <div className="agent-quota-label" key={`${balance.label}:${balance.currency}`}><span>{balance.label}</span><strong>{formatProviderBalance(balance.amount, balance.currency, lang)}</strong></div>)}
            </div>}
            {quota.windows.length > 0 && <div className="agent-quota-caption">{t("agentUi.remainingQuota", undefined, lang)}</div>}
            <div className="agent-quota-windows">
              {quota.windows.map(window => {
                const reset = quotaWindowHasReset(window);
                const remaining = reset ? null : remainingPercent(window.usedPercent);
                const label = `${quotaScopeLabel(window.scope) ? `${quotaScopeLabel(window.scope)} · ` : ""}${quotaWindowLabel(window.windowMinutes, window.id, lang)}`;
                const resetLabel = window.resetsAtUnix ? t("agentUi.resetAt", { time: formatResetTime(window.resetsAtUnix, Date.now(), lang) }, lang) : quotaNativeResetLabel(quota, window, lang);
                return <div className="agent-quota-window" key={window.id}>
                  <div className="agent-quota-label"><span>{label}</span><strong style={remaining === 0 ? { color: quotaColor(0) } : undefined}>{reset ? t("agentUi.refreshPending", undefined, lang) : remaining == null ? t("agentUi.unknown", undefined, lang) : formatQuotaPercent(remaining, lang)}</strong></div>
                  <div className="agent-quota-meter" role={remaining == null ? undefined : "progressbar"} aria-label={`${label}${lang.startsWith("zh") ? "" : " "}${t("agentUi.remainingQuota", undefined, lang)}`} aria-valuemin={remaining == null ? undefined : 0} aria-valuemax={remaining == null ? undefined : 100} aria-valuenow={remaining ?? undefined} data-unknown={remaining == null || undefined}>
                    {remaining != null && <i style={{ width: `${remaining}%`, background: quotaColor(remaining) }} />}
                  </div>
                  <small className="agent-quota-reset">{reset ? t("agentUi.resetPending", undefined, lang) : remaining == null ? t("agentUi.usageUnknown", undefined, lang) : remaining === 0 ? t("agentUi.usedUp", undefined, lang) : ""}{!reset && resetLabel && <span>{resetLabel}</span>}</small>
                </div>;
              })}
            </div>
            {((!quota.windows.length && !quota.balances?.length) || quotaShowsLastReading(view.diagnostic)) && <p className="agent-quota-notice" data-outdated={quota.windows.length > 0 || undefined}>{quotaDiagnostic(view.diagnostic, lang)}{quota.windows.length > 0 && t("agentUi.previousReading", undefined, lang)}</p>}
            {quota.context && <div className="agent-context"><span>{t("agentUi.sessionContext", undefined, lang)}</span><b>{quota.context.usedTokens.toLocaleString(lang)} / {quota.context.windowTokens.toLocaleString(lang)}</b>{quota.context.cacheHitRatePercent != null && <small>{t("agentUi.cacheHit", { percent: new Intl.NumberFormat(lang, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(quota.context.cacheHitRatePercent) }, lang)}</small>}</div>}
          </> : <p className="agent-quota-notice">{quotaDiagnostic(view.diagnostic, lang)}</p>}
        </div>
        {installed && <footer className="agent-card-footer">
          <div className="agent-card-version" data-tooltip={versionFor(view.installationId)} data-tooltip-overflow="">{versionFor(view.installationId)}</div>
          <AgentUpdateControl id={view.installationId} actions={agentUpdates} />
        </footer>}
      </section>;
    })}
  </div>;
  const status = management && !host && <span className="agent-order-status" role="status" aria-live="polite">{announcement}</span>;
  if (!uninstalled.length) return <>{cards}{status}</>;
  return <>{cards}{status}
    <section className="agent-uninstalled" aria-labelledby="agent-uninstalled-title">
      <header><h3 id="agent-uninstalled-title">{t("agentUi.notInstalled", undefined, lang)}</h3><span>{t(host ? "agentUi.installOnHostHint" : "agentUi.installRescanHint", undefined, lang)}</span>
        {onDetect && <button className="agent-detect-button" type="button" disabled={detecting} aria-busy={detecting}
          onClick={() => { setDetecting(true); void onDetect(uninstalled.map(agent => agent.id)).finally(() => setDetecting(false)); }}>
          {detecting && <span className="task-running-spinner" aria-hidden="true" />}{detecting ? t("agentUi.rescanning", undefined, lang) : t("agentUi.rescan", undefined, lang)}
        </button>}
      </header>
      <ul>{uninstalled.map(agent => {
        const install = host?.installs.find(item => item.providerId === agent.id);
        return <li key={agent.id} aria-label={agent.name}>
        <AgentIcon providerId={agent.id} /><span className="agent-uninstalled-name">{agent.name}</span>
        {host
          ? <button type="button" className="agent-install-link" disabled={install?.status === "installing"} aria-busy={install?.status === "installing"} aria-label={t("agentUi.installOnHost", { name: agent.name }, lang)} onClick={() => host.install(agent.id)}>
              {install?.status === "installing" && <span className="task-running-spinner" aria-hidden="true" />}
              {t(install?.status === "installing" ? "agentUi.installing" : install ? "agentUi.retryInstall" : "agentUi.install", undefined, lang)}
            </button>
          : <InstallationLink url={agent.installUrl} name={agent.name} className="agent-install-link" errorClassName="agent-install-error" />}
        {install?.status === "failed" && <span className="agent-install-error" role="alert">{localizedAgentMessage(install.message, "installFailed", lang)}</span>}
      </li>;
      })}</ul>
    </section></>;
}

