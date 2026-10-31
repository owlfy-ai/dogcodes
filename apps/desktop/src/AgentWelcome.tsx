import { t, useI18n, type ResolvedLanguage } from "./i18n";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { localizedAgentError } from "./agentI18n";
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { BootstrapSnapshot, CoreClient, InstallationSummary, QuotaSnapshotView, QuotaWindow } from "@dogcodes/protocol";
import { quotaScopeLabel, quotaColor, quotaIsStale, quotaNativeResetLabel, quotaWindowHasReset, quotaWindowLabel, formatResetTime, remainingPercent, formatProviderBalance } from "./quotaPresentation";
import { quotaRequestTimeoutMs } from "./quotaRefresh";
import { quotaShowsLastReading, withLastReading } from "./quotaFallback";
import { AgentLogin, agentAccountIssue, needsAgentLogin } from "./AgentLogin";
import { WelcomeHerd } from "./WelcomeHerd";
import "./agentWelcome.css";

export const supportedAgents = [
  { id: "codex", installUrl: "https://developers.openai.com/codex/cli/", name: "Codex" },
  { id: "claude-code", installUrl: "https://code.claude.com/docs/en/setup", name: "Claude Code" },
  { id: "grok", installUrl: "https://docs.x.ai/build/overview", name: "Grok Build" },
  { id: "cursor", installUrl: "https://cursor.com/docs/cli/overview", name: "Cursor" },
  { id: "antigravity", installUrl: "https://antigravity.google/download", name: "Antigravity" },
  { id: "zcode", installUrl: "https://zcode.z.ai/en/docs/install", name: "ZCode" },
  { id: "kimi", installUrl: "https://moonshotai.github.io/kimi-code/", name: "Kimi Code" },
  { id: "codebuddy", installUrl: "https://www.codebuddy.cn/docs/cli/installation", name: "CodeBuddy" },
  { id: "pi", installUrl: "https://pi.dev/docs/latest/quickstart", name: "Pi" },
  { id: "opencode", installUrl: "https://opencode.ai/docs/", name: "OpenCode" },
  { id: "deepseek-harness", installUrl: "https://github.com/deepseek-ai/deepseek-harness#run", name: "DeepSeek Harness" },
  { id: "kiro", installUrl: "https://kiro.dev/downloads/", name: "Kiro" },
] as const;
const withoutQuota = new Set(["codebuddy"]);
const independentAccounts = new Set(["pi", "opencode", "deepseek-harness"]);
const canReadQuota = (installation: InstallationSummary) => !withoutQuota.has(installation.providerId)
  && (installation.status === "ready" || (independentAccounts.has(installation.providerId)
    && installation.status !== "unsupported_version"));

type Reading = { installation: InstallationSummary; view?: QuotaSnapshotView; pending?: boolean; failed?: boolean; observedAt?: number };
export function welcomeStatus(reading?: Reading, lang: ResolvedLanguage = "zh-CN"): { label: string; tone: string; detail: string } {
  if (!reading) return { label: t("agentUi.notInstalled", undefined, lang), tone: "muted", detail: t("agentUi.installHint", undefined, lang) };
  const { installation: i, view, pending, failed } = reading;
  if (view?.quota?.confidence === "demo" || i.version === "demo") return { label: t("agentUi.demo", undefined, lang), tone: "muted", detail: t("agentUi.demoHint", undefined, lang) };
  const accountIssue = agentAccountIssue(i, view, lang);
  if (accountIssue) return { label: accountIssue.label, tone: "warning", detail: accountIssue.detail };
  if (i.status === "unsupported_version") return { label: t("agentUi.incompatible", undefined, lang), tone: "warning", detail: t("agentUi.incompatibleHint", undefined, lang) };
  if (i.status === "broken") return { label: t("agentUi.connectionFailed", undefined, lang), tone: "warning", detail: t("agentUi.connectionFailedHint", undefined, lang) };
  if (pending) return { label: t("agentUi.verifying", undefined, lang), tone: "muted", detail: t("agentUi.queryingHint", undefined, lang) };
  if (view?.diagnostic === "quota_exhausted")
    return { label: t("agentUi.exhausted", undefined, lang), tone: "warning", detail: t("agentUi.exhaustedHint", undefined, lang) };
  if (view?.diagnostic === "claude_quota_token_expired")
    return { label: t("agentUi.tokenExpired", undefined, lang), tone: "warning", detail: t("agentUi.tokenExpiredHint", undefined, lang) };
  if (quotaShowsLastReading(view?.diagnostic))
    return { label: t("agentUi.rateLimited", undefined, lang), tone: "warning", detail: view?.quota?.windows.length
      ? t("agentUi.lastReadingHint", { time: new Date(view.quota.collectedAtUnixMs).toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" }) }, lang)
      : t("agentUi.rateLimitedHint", undefined, lang) };
  if (view?.diagnostic === "quota_network_error")
    return { label: t("agentUi.networkError", undefined, lang), tone: "warning", detail: t("agentUi.networkErrorHint", undefined, lang) };
  if (view?.diagnostic === "quota_timeout")
    return { label: t("agentUi.timeout", undefined, lang), tone: "warning", detail: t("agentUi.timeoutHint", undefined, lang) };
  if (view?.diagnostic === "claude_quota_custom_provider")
    return { label: t("agentUi.customProvider", undefined, lang), tone: "third-party", detail: t("agentUi.customProviderHint", undefined, lang) };
  if (view?.diagnostic === "opencode_go_not_configured")
    return { label: i.status === "ready" ? t("agentUi.connected", undefined, lang) : t("agentUi.goMissing", undefined, lang), tone: i.status === "ready" ? "ready" : "warning", detail: t("agentUi.goMissingHint", undefined, lang) };
  // Authentication is an observation made when this run receives its response.
  // Expiring a quota reading later is not a new authentication failure.
  if (i.status === "ready" && view?.quota?.windows.length
    && !quotaIsStale(view.quota.staleAfterUnixMs, reading.observedAt ?? Date.now())
    && view.quota.confidence === "provider_reported")
    return { label: t("agentUi.loggedIn", undefined, lang), tone: "ready", detail: quotaIsStale(view.quota.staleAfterUnixMs)
      ? t("agentUi.quotaVerifiedExpired", undefined, lang)
      : t("agentUi.quotaVerified", undefined, lang) };
  if (i.status === "ready" && view?.quota?.balances?.length
    && !quotaIsStale(view.quota.staleAfterUnixMs, reading.observedAt ?? Date.now())
    && view.quota.confidence === "provider_reported")
    return { label: t("agentUi.balanceVerified", undefined, lang), tone: "ready", detail: quotaIsStale(view.quota.staleAfterUnixMs)
      ? t("agentUi.balanceVerifiedExpired", undefined, lang)
      : t("agentUi.balanceVerifiedHint", undefined, lang) };
  if (withoutQuota.has(i.providerId) && i.status === "ready")
    return { label: t("agentUi.connected", undefined, lang), tone: "ready", detail: t("agentUi.unsupportedQuotaHint", undefined, lang) };
  if (independentAccounts.has(i.providerId) && i.status === "ready" && !failed)
    return { label: t("agentUi.connected", undefined, lang), tone: "ready", detail: t("agentUi.noBalanceHint", undefined, lang) };
  return { label: failed ? t("agentUi.detectionFailed", undefined, lang) : t("agentUi.authPending", undefined, lang), tone: "warning", detail: t("agentUi.noReadingHint", undefined, lang) };
}

function WindowRing({ window, quota, scope = "" }: { window?: QuotaWindow; quota?: QuotaSnapshotView["quota"]; scope?: string }) {
  const { resolvedLanguage: lang } = useI18n();
  const expired = window && quotaWindowHasReset(window);
  const remaining = expired ? null : remainingPercent(window?.usedPercent);
  const label = window ? quotaWindowLabel(window.windowMinutes, window.id, lang) : t("agentUi.remainingQuota", undefined, lang);
  const reset = window?.resetsAtUnix
    ? expired ? t("agentUi.resetPending", undefined, lang) : t("agentUi.resetAt", { time: formatResetTime(window.resetsAtUnix, Date.now(), lang) }, lang)
    : window && quota ? quotaNativeResetLabel(quota, window, lang) || t("agentUi.resetMissing", undefined, lang) : "";
  return <div className="welcome-window">
    <div className="welcome-ring" role="img" aria-label={`${quotaScopeLabel(window?.scope)} ${label}${lang.startsWith("zh") ? "：" : ": "}${remaining == null ? t("agentUi.unknown", undefined, lang) : t("agentUi.remainingPercent", { percent: new Intl.NumberFormat(lang, { maximumFractionDigits: 1 }).format(remaining!) }, lang)}`}>
      <svg viewBox="0 0 100 100" aria-hidden="true"><circle className="welcome-ring-track" cx="50" cy="50" r="42" />
        {remaining != null && <circle cx="50" cy="50" r="42" pathLength="100" stroke={quotaColor(remaining)} strokeDasharray={`${remaining} 100`} className="welcome-ring-value" />}</svg>
      <span><b>{remaining == null ? "—" : remaining > 0 && remaining < 1 ? "<1" : Math.round(remaining)}{remaining != null && <small>%</small>}</b><em>{remaining === 0 ? t("agentUi.usedUp", undefined, lang) : t("agentUi.remaining", undefined, lang)}</em></span>
    </div>
    <div className="welcome-window-copy">
      <span className="welcome-window-title"><strong>{window ? t("agentUi.windowRemaining", { window: lang === "zh-CN" || lang === "zh-TW" ? label.replace(/\s+/g, "") : label }, lang) : label}</strong>{scope && <em>{scope}</em>}</span>
      {reset && <small>{reset}</small>}
    </div>
  </div>;
}

// Bound the UI wait as well as the native probe: hot reload can orphan an IPC reply.
function awaitDetection<T>(request: Promise<T>, signal: AbortSignal, timeoutMs: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const finish = (action: () => void) => {
      clearTimeout(timer);
      signal.removeEventListener("abort", abort);
      action();
    };
    const abort = () => finish(() => reject(new Error("detection cancelled")));
    const timer = setTimeout(() => finish(() => reject(new Error("detection timed out"))), timeoutMs);
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) abort();
    request.then(value => finish(() => resolve(value)), error => finish(() => reject(error)));
  });
}

export function InstallationLink({ url, name, className = "welcome-install-link", errorClassName = "welcome-error" }: { url: string; name: string; className?: string; errorClassName?: string }) {
  const { resolvedLanguage: lang } = useI18n();
  const [error, setError] = useState(false);
  return <><a className={className} href={url} target="_blank" rel="noopener noreferrer"
    aria-label={t("agentUi.installWebsite", { name }, lang)}
    onClick={event => {
      if (!isTauri()) return;
      event.preventDefault(); setError(false);
      void invoke("open_website", { url }).catch(() => setError(true));
    }}>{t("agentUi.install", undefined, lang)} <span aria-hidden="true">↗</span></a>
    {error && <span className={errorClassName} role="alert">{t("agentUi.browserFailed", undefined, lang)}</span>}</>;
}

export function AgentWelcome({ client, onContinue, renderIcon, onDetected }: {
  client: CoreClient; onContinue: () => void; renderIcon: (id: string) => ReactNode;
  onDetected?: (provider: string, snapshot: BootstrapSnapshot) => void;
}) {
  const { resolvedLanguage: lang } = useI18n();
  const [readings, setReadings] = useState<Reading[]>([]);
  const [pendingAgents, setPendingAgents] = useState<string[]>(supportedAgents.map(a => a.id));
  const [failedAgents, setFailedAgents] = useState<string[]>([]);
  const [failureDetails, setFailureDetails] = useState<Record<string, unknown>>({});
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(false);
  const busy = useRef(false);
  const generation = useRef(0);
  const activeRun = useRef<AbortController | null>(null);
  const detectedRef = useRef(onDetected); detectedRef.current = onDetected;
  const run = async () => {
    if (busy.current) return;
    busy.current = true;
    const runId = ++generation.current;
    const controller = new AbortController();
    activeRun.current = controller;
    const current = () => mounted.current && generation.current === runId;
    setPendingAgents(supportedAgents.map(a => a.id)); setFailedAgents([]);
    setReadings([]); setFailureDetails({}); setRefreshing(true); setError(null);
    const pendingReads = new Map<string, Promise<void>>();
    const scanned = new Set<string>();
    const readQuota = (installation: InstallationSummary): Promise<void> => {
      const id = installation.installationId;
      const pending = pendingReads.get(id);
      if (pending) return pending;
      const request = (async () => {
        let view: QuotaSnapshotView | undefined;
        let failed = false;
        try {
          const found = (await awaitDetection(client.quotaSnapshots(null, id), controller.signal, quotaRequestTimeoutMs(installation.providerId)))
            .find(v => v.installationId === id && v.providerId === installation.providerId);
          view = found && withLastReading(found);
          failed = !found;
        } catch { failed = true; }
        const observedAt = Date.now();
        if (current()) setReadings(previous => previous.map(r => r.installation.installationId === id ? { ...r, view, failed, observedAt, pending: false } : r));
      })();
      pendingReads.set(id, request);
      void request.finally(() => pendingReads.delete(id));
      return request;
    };
    // Account APIs do not depend on a successful model/session handshake. Read
    // known installations while discovery runs, including when a scan is blocked.
    const accounts = (async () => {
      if (!client.bootstrap) return;
      try {
        const snapshot = await awaitDetection(client.bootstrap(), controller.signal, 35_000);
        if (!current()) return;
        const installations = snapshot.installations.filter(i => independentAccounts.has(i.providerId)
          && canReadQuota(i) && !scanned.has(i.providerId));
        setReadings(previous => [...previous, ...installations.filter(i => !previous.some(r => r.installation.installationId === i.installationId))
          .map(installation => ({ installation, pending: true }))]);
        await Promise.all(installations.map(readQuota));
      } catch { /* Discovery still supplies installations when bootstrap fails. */ }
    })();
    try {
      await Promise.all([accounts, ...supportedAgents.map(async agent => {
        try {
          const snapshot = await awaitDetection(client.scanAgent(agent.id), controller.signal, 100_000);
          if (!current()) return;
          const installations = snapshot.installations.filter(i => i.providerId === agent.id);
          scanned.add(agent.id);
          detectedRef.current?.(agent.id, snapshot);
          setReadings(previous => [...previous.filter(r => r.installation.providerId !== agent.id),
            ...installations.map(installation => ({ ...previous.find(r => r.installation.installationId === installation.installationId), installation, pending: canReadQuota(installation) }))]);
          setPendingAgents(previous => previous.filter(id => id !== agent.id));
          await Promise.all(installations.filter(canReadQuota).map(readQuota));
          if (current()) setReadings(previous => previous.map(r => r.installation.providerId === agent.id ? { ...r, pending: false } : r));
        } catch (cause) {
          if (current()) {
            setFailureDetails(previous => ({ ...previous, [agent.id]: cause }));
            setFailedAgents(previous => [...previous, agent.id]);
            setPendingAgents(previous => previous.filter(id => id !== agent.id));
            setError("agentUi.partialFailure");
          }
        }
      })]);
    } finally {
      if (current()) { busy.current = false; activeRun.current = null; setRefreshing(false); }
    }
  };
  useEffect(() => {
    mounted.current = true;
    void run();
    return () => {
      mounted.current = false;
      generation.current++;
      activeRun.current?.abort();
      activeRun.current = null;
      busy.current = false;
    };
  }, [client]);
  // Keep reset/expiry labels honest while the welcome page remains open.
  const [, tick] = useState(0);
  useEffect(() => { const timer = setInterval(() => tick(n => n + 1), 30_000); return () => clearInterval(timer); }, []);
  const installedAgents = new Set(readings.map(reading => reading.installation.providerId));
  const installationRank = (id: string) => installedAgents.has(id) ? 0
    : pendingAgents.includes(id) || failedAgents.includes(id) ? 1 : 2;
  const orderedAgents = [...supportedAgents].sort((left, right) => installationRank(left.id) - installationRank(right.id));
  const cards = orderedAgents.map(agent => {
    const reading = readings.find(r => r.installation.providerId === agent.id);
    const scanning = pendingAgents.includes(agent.id) && !reading;
    const failed = failedAgents.includes(agent.id);
    const uninstalled = !reading && !scanning && !failed;
    const state = scanning ? { label: t("agentUi.scanning", undefined, lang), tone: "muted", detail: t("agentUi.scanningHint", undefined, lang) }
      : failed ? { label: t("agentUi.detectionFailed", undefined, lang), tone: "warning", detail: t("agentUi.incompleteHint", undefined, lang) } : welcomeStatus(reading, lang);
    return { agent, reading, scanning, failed, uninstalled, state };
  });
  const connected = cards.filter(card => !card.uninstalled);
  const available = cards.filter(card => card.uninstalled);
  const readyCount = cards.filter(card => card.reading && card.state.tone === "ready").length;
  const attentionCount = cards.filter(card => card.failed || (card.reading && ["warning", "third-party"].includes(card.state.tone))).length;
  const scanningCount = pendingAgents.length;
  const logo = new URL("../src-tauri/icons/128x128.png", import.meta.url).href;
  const rescan = <button className="welcome-rescan" type="button" disabled={refreshing} onClick={() => void run()}>
    {refreshing ? <><span className="welcome-spinner" aria-hidden="true" />{t("agentUi.rescanning", undefined, lang)}</> : <><RefreshIcon />{t("agentUi.rescan", undefined, lang)}</>}
  </button>;

  const renderCard = ({ agent, reading, scanning, failed, uninstalled, state }: typeof cards[number], index: number) => {
    if (uninstalled) return <article className="welcome-agent-card uninstalled" key={agent.id} aria-label={agent.name} style={{ "--stagger": index } as CSSProperties}>
      <header><span className="welcome-agent-icon">{renderIcon(agent.id)}</span><div><h2>{agent.name}</h2><small>{t("agentUi.notInstalled", undefined, lang)}</small></div></header>
      <InstallationLink url={agent.installUrl} name={agent.name} />
    </article>;
    const quota = reading?.view?.quota;
    const displayWindows = (quota?.windows ?? []).filter(w => agent.id !== "antigravity"
      || ((w.scope === "Gemini Models" || w.id.startsWith("Gemini Models:"))
        && (w.windowMinutes === 300 || w.windowMinutes === 10_080)));
    const groups = [...new Set(displayWindows.map(w => w.scope ?? ""))];
    // A reading that could not be refreshed is shown greyed with its time, never as current.
    const outdated = !scanning && displayWindows.length > 0 && quotaShowsLastReading(reading?.view?.diagnostic);
    const busy = scanning || Boolean(reading?.pending);
    const emptyMessage = scanning || reading?.pending ? t("agentUi.waiting", undefined, lang) : failed ? t("agentUi.incomplete", undefined, lang)
      : withoutQuota.has(agent.id) ? t("agentUi.quotaUnsupported", undefined, lang)
      : reading?.view?.diagnostic === "opencode_go_not_configured" ? t("agentUi.goMissingEmpty", undefined, lang)
      : independentAccounts.has(agent.id) ? (reading?.failed ? t("agentUi.quotaFailed", undefined, lang) : state.tone === "warning" ? state.detail : t("agentUi.noBalance", undefined, lang))
      : t("agentUi.noQuota", undefined, lang);
    return <article className={`welcome-agent-card ${state.tone}${busy ? " busy" : ""}`} key={agent.id} aria-label={agent.name} style={{ "--stagger": index } as CSSProperties}>
      <header>
        <span className="welcome-agent-icon">{renderIcon(agent.id)}</span>
        <div><h2>{agent.name}</h2><small>{reading ? t("agentUi.installed", undefined, lang) : scanning ? t("agentUi.finding", undefined, lang) : t("agentUi.installationPending", undefined, lang)}{outdated && quota && ` · ${t("agentUi.lastReading", { time: new Date(quota.collectedAtUnixMs).toLocaleString(lang, { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }) }, lang)}`}</small></div>
        <span className={`welcome-status ${state.tone}`} data-tooltip={failed ? localizedAgentError(failureDetails[agent.id], "agentUi.scanUnavailable", lang) : state.detail} aria-busy={busy}>
          {busy ? <span className="welcome-spinner" aria-hidden="true" /> : <i aria-hidden="true" />}{state.label}
          {!scanning && !failed && reading && needsAgentLogin(reading.installation, reading.view) &&
            <AgentLogin key={reading.installation.installationId} installationId={reading.installation.installationId} name={agent.name} />}
        </span>
      </header>
      <div className={`welcome-card-quotas${outdated ? " outdated" : ""}`}>
        {!scanning && (displayWindows.length > 0 || !!quota?.balances?.length) ? <>
          {groups.map(group => <section key={group} aria-label={quotaScopeLabel(group) || t("agentUi.accountQuota", undefined, lang)}>
          <div className="welcome-rings">{displayWindows.filter(w => (w.scope ?? "") === group)
            .map(w => <WindowRing key={w.id} window={w} quota={quota} scope={agent.id !== "antigravity" ? quotaScopeLabel(group) : ""} />)}</div>
          </section>)}
          {!!quota?.balances?.length && <section className="welcome-provider-balances" aria-label={t("agentUi.providerBalance", undefined, lang)}>
            {quota.balances.map(balance => <div className="welcome-provider-balance" key={`${balance.label}:${balance.currency}`}>
              <span>{balance.label}</span><strong>{formatProviderBalance(balance.amount, balance.currency, lang)}</strong>
            </div>)}
            <small>{quotaIsStale(quota.staleAfterUnixMs) ? `${t("agentUi.stale", undefined, lang)} · ` : ""}{t("agentUi.updatedAt", { time: new Date(quota.collectedAtUnixMs).toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" }) }, lang)}</small>
          </section>}
        </> : <div className={`welcome-empty-quota${busy ? " loading" : ""}`}>
          {busy && <span className="welcome-skeleton" aria-hidden="true"><i /><i /></span>}
          <p>{emptyMessage}</p>
        </div>}
      </div>
      {reading && <footer>
        <details><summary>{t("agentUi.installationPath", undefined, lang)}</summary><code>{reading.installation.executable}</code></details>
      </footer>}
    </article>;
  };

  return <main className="agent-welcome">
    <header className="welcome-topbar" data-tauri-drag-region="deep">
      <span className="welcome-brand"><img src={logo} alt="" />dog.codes</span>
      <button className="welcome-skip" type="button" onClick={onContinue}>{t("agentUi.skip", undefined, lang)} <span aria-hidden="true">→</span></button>
    </header>
    <div className="welcome-page-body">
      <section className="welcome-hero">
        <div className="welcome-hero-copy">
          <p className="welcome-eyebrow"><span aria-hidden="true" />{t("agentUi.welcome", undefined, lang)}</p>
          <h1>{t("agentUi.heroFirst", undefined, lang)}<br />{t("agentUi.heroSecond", undefined, lang)}</h1>
          <p className="welcome-lede">{t("agentUi.intro", undefined, lang)}</p>
          <div className="welcome-actions">
            <button className="welcome-primary" type="button" onClick={onContinue}>{t("agentUi.start", undefined, lang)} <span aria-hidden="true">→</span></button>
            {rescan}
          </div>
        </div>
        <div className="welcome-hero-visual">
          <WelcomeHerd renderIcon={renderIcon} agents={supportedAgents.map(agent => {
            const card = cards.find(item => item.agent.id === agent.id)!;
            return { id: agent.id, tone: card.reading ? "installed" : card.scanning ? "scanning" : card.failed ? "failed" : "missing" };
          })} />
          <dl className="welcome-stats" aria-live="polite">
            <div className="ready"><dt>{t("agentUi.ready", undefined, lang)}</dt><dd>{readyCount}</dd></div>
            <div className="attention"><dt>{t("agentUi.attention", undefined, lang)}</dt><dd>{attentionCount}</dd></div>
            <div><dt>{scanningCount ? t("agentUi.scanning", undefined, lang) : t("agentUi.addable", undefined, lang)}</dt><dd>{scanningCount || available.length}</dd></div>
          </dl>
        </div>
      </section>

      {error && <p className="welcome-error" role="alert">{t(error, undefined, lang)}</p>}

      <section className="welcome-section" aria-labelledby="welcome-connected-title">
        <header className="welcome-section-heading">
          <div><h2 id="welcome-connected-title">{t("agentUi.yourAgents", undefined, lang)}<span className="welcome-count">{connected.length}</span></h2><p>{refreshing ? t("agentUi.checkingConnections", undefined, lang) : t("agentUi.discovered", { count: installedAgents.size }, lang)}</p></div>
        </header>
        {connected.length ? <div className="welcome-agent-grid">{connected.map(renderCard)}</div>
          : <div className="welcome-none"><b>{t("agentUi.noneFound", undefined, lang)}</b><p>{t("agentUi.noneFoundHint", undefined, lang)}</p></div>}
      </section>

      {available.length > 0 && <section className="welcome-section" aria-labelledby="welcome-available-title">
        <header className="welcome-section-heading">
          <div><h2 id="welcome-available-title">{t("agentUi.moreAgents", undefined, lang)}<span className="welcome-count">{available.length}</span></h2><p>{t("agentUi.moreAgentsHint", undefined, lang)}</p></div>
        </header>
        <div className="welcome-available-grid">{available.map(renderCard)}</div>
      </section>}

    </div>
  </main>;
}

function RefreshIcon() {
  return <svg className="welcome-button-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3" /></svg>;
}
