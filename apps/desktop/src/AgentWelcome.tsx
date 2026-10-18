import { invoke, isTauri } from "@tauri-apps/api/core";
import { userFacingError } from "./userFacingError";
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { BootstrapSnapshot, CoreClient, InstallationSummary, QuotaSnapshotView, QuotaWindow } from "@dogcodes/protocol";
import { quotaScopeLabel, quotaColor, quotaIsStale, quotaNativeResetLabel, quotaWindowHasReset, quotaWindowLabel, formatResetTime, remainingPercent } from "./quotaPresentation";
import { quotaRequestTimeoutMs } from "./quotaRefresh";
import { quotaShowsLastReading, withLastReading } from "./quotaFallback";
import { AgentLogin, agentAccountIssue, needsAgentLogin } from "./AgentLogin";
import { WelcomeHerd } from "./WelcomeHerd";
import "./agentWelcome.css";

export const supportedAgents = [
  { id: "codex", installUrl: "https://developers.openai.com/codex/cli/", name: "Codex", login: "codex login" },
  { id: "claude-code", installUrl: "https://code.claude.com/docs/en/setup", name: "Claude Code", login: "claude auth login" },
  { id: "grok", installUrl: "https://docs.x.ai/build/overview", name: "Grok Build", login: "打开终端运行 grok login" },
  { id: "cursor", installUrl: "https://cursor.com/docs/cli/overview", name: "Cursor", login: "cursor-agent login" },
  { id: "antigravity", installUrl: "https://antigravity.google/download", name: "Antigravity", login: "在 Antigravity 中登录" },
  { id: "zcode", installUrl: "https://zcode.z.ai/en/docs/install", name: "ZCode", login: "在 ZCode 中登录并选择模型" },
  { id: "kimi", installUrl: "https://moonshotai.github.io/kimi-code/", name: "Kimi Code", login: "打开终端运行 kimi login" },
  { id: "codebuddy", installUrl: "https://www.codebuddy.cn/docs/cli/installation", name: "CodeBuddy", login: "在 CodeBuddy 中登录" },
  { id: "pi", installUrl: "https://pi.dev/docs/latest/quickstart", name: "Pi", login: "在 Pi 中运行 /login，或配置模型服务" },
  { id: "opencode", installUrl: "https://opencode.ai/docs/", name: "OpenCode", login: "opencode auth login" },
  { id: "deepseek-harness", installUrl: "https://github.com/deepseek-ai/deepseek-harness#run", name: "DeepSeek Harness", login: "在 DeepSeek Harness 中配置模型服务" },
  { id: "kiro", installUrl: "https://kiro.dev/downloads/", name: "Kiro", login: "kiro-cli login" },
] as const;
const withoutQuota = new Set(["codebuddy", "pi", "opencode", "deepseek-harness"]);

type Reading = { installation: InstallationSummary; view?: QuotaSnapshotView; pending?: boolean; failed?: boolean; observedAt?: number };
export function welcomeStatus(reading?: Reading): { label: string; tone: string; detail: string } {
  if (!reading) return { label: "未安装", tone: "muted", detail: "安装后重新检测，即可连接本机 Agent。" };
  const { installation: i, view, pending, failed } = reading;
  if (view?.quota?.confidence === "demo" || i.version === "demo") return { label: "演示数据", tone: "muted", detail: "仅用于界面预览，不代表本机安装或真实账户。" };
  const accountIssue = agentAccountIssue(i, view);
  if (accountIssue) return { label: accountIssue.label, tone: "warning", detail: accountIssue.detail };
  if (i.status === "unsupported_version") return { label: "暂不兼容", tone: "warning", detail: "本机客户端暂不兼容，可先进入工作台使用其他 Agent。" };
  if (i.status === "broken") return { label: "连接失败", tone: "warning", detail: "原生连接检测未通过，请检查客户端及网络后重试。" };
  if (pending) return { label: "验证授权中", tone: "muted", detail: "正在查询账户额度与重置时间…" };
  if (view?.diagnostic === "quota_exhausted")
    return { label: "额度已用尽", tone: "warning", detail: "原生服务报告额度已用尽，请等待额度重置或检查套餐后重新检测。" };
  if (view?.diagnostic === "claude_quota_token_expired")
    return { label: "令牌待续期", tone: "warning", detail: "Claude Code 的短期访问令牌已到期，登录状态仍有效，无需重新登录。下次在终端或工作台用 Claude Code 对话时会自动续期。" };
  if (quotaShowsLastReading(view?.diagnostic))
    return { label: "额度查询受限", tone: "warning", detail: view?.quota?.windows.length
      ? `原生额度服务暂时限制请求，显示的是 ${new Date(view.quota.collectedAtUnixMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })} 的读数，稍后自动重试。`
      : "原生额度服务暂时限制请求，请稍后重新检测。" };
  if (view?.diagnostic === "quota_network_error")
    return { label: "额度查询网络异常", tone: "warning", detail: "未能连接原生额度服务，请检查网络和系统代理后重试。" };
  if (view?.diagnostic === "quota_timeout")
    return { label: "额度查询超时", tone: "warning", detail: "原生连接已就绪，但额度查询超时。请稍后重新检测。" };
  if (view?.diagnostic === "claude_quota_custom_provider")
    return { label: "第三方模型", tone: "third-party", detail: "原生连接已通过；自定义服务凭证及额度尚未验证。" };
  // Authentication is an observation made when this run receives its response.
  // Expiring a quota reading later is not a new authentication failure.
  if (i.status === "ready" && view?.quota?.windows.length
    && !quotaIsStale(view.quota.staleAfterUnixMs, reading.observedAt ?? Date.now())
    && view.quota.confidence === "provider_reported")
    return { label: "已登录", tone: "ready", detail: quotaIsStale(view.quota.staleAfterUnixMs)
      ? "本轮检测时授权有效 · 额度读数已过期，可重新检测"
      : "原生连接已通过 · 账户额度已验证" };
  if (withoutQuota.has(i.providerId) && i.status === "ready")
    return { label: "连接就绪", tone: "ready", detail: "原生会话检测已通过 · 暂不支持额度查询" };
  return { label: failed ? "检测失败" : "授权待确认", tone: "warning", detail: "未取得有效账户读数。请检查网络或原生登录状态后重试。" };
}

function WindowRing({ window, quota, scope = "" }: { window?: QuotaWindow; quota?: QuotaSnapshotView["quota"]; scope?: string }) {
  const expired = window && quotaWindowHasReset(window);
  const remaining = expired ? null : remainingPercent(window?.usedPercent);
  const label = window ? quotaWindowLabel(window.windowMinutes, window.id) : "剩余额度";
  const reset = window?.resetsAtUnix
    ? expired ? "已到重置时间，待刷新" : `重置时间：${formatResetTime(window.resetsAtUnix)}`
    : window && quota ? quotaNativeResetLabel(quota, window) || "重置时间未提供" : "";
  return <div className="welcome-window">
    <div className="welcome-ring" role="img" aria-label={`${quotaScopeLabel(window?.scope)} ${label}：${remaining == null ? "未知" : `剩余 ${remaining}%`}`}>
      <svg viewBox="0 0 100 100" aria-hidden="true"><circle className="welcome-ring-track" cx="50" cy="50" r="42" />
        {remaining != null && <circle cx="50" cy="50" r="42" pathLength="100" stroke={quotaColor(remaining)} strokeDasharray={`${remaining} 100`} className="welcome-ring-value" />}</svg>
      <span><b>{remaining == null ? "—" : remaining > 0 && remaining < 1 ? "<1" : Math.round(remaining)}{remaining != null && <small>%</small>}</b><em>{remaining === 0 ? "已用尽" : "剩余"}</em></span>
    </div>
    <div className="welcome-window-copy">
      <span className="welcome-window-title"><strong>{window ? `${label.replace(/\s+/g, "")}剩余` : label}</strong>{scope && <em>{scope}</em>}</span>
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
  const [error, setError] = useState(false);
  return <><a className={className} href={url} target="_blank" rel="noopener noreferrer"
    aria-label={`${name}：前往官网安装`}
    onClick={event => {
      if (!isTauri()) return;
      event.preventDefault(); setError(false);
      void invoke("open_website", { url }).catch(() => setError(true));
    }}>安装 <span aria-hidden="true">↗</span></a>
    {error && <span className={errorClassName} role="alert">无法打开浏览器，请重试或复制链接。</span>}</>;
}

export function AgentWelcome({ client, onContinue, renderIcon, onDetected }: {
  client: CoreClient; onContinue: () => void; renderIcon: (id: string) => ReactNode;
  onDetected?: (provider: string, snapshot: BootstrapSnapshot) => void;
}) {
  const [readings, setReadings] = useState<Reading[]>([]);
  const [pendingAgents, setPendingAgents] = useState<string[]>(supportedAgents.map(a => a.id));
  const [failedAgents, setFailedAgents] = useState<string[]>([]);
  const [failureDetails, setFailureDetails] = useState<Record<string, string>>({});
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
    try {
      await Promise.all(supportedAgents.map(async agent => {
        try {
          const snapshot = await awaitDetection(client.scanAgent(agent.id), controller.signal, 100_000);
          if (!current()) return;
          const installations = snapshot.installations.filter(i => i.providerId === agent.id);
          detectedRef.current?.(agent.id, snapshot);
          setReadings(previous => [...previous.filter(r => r.installation.providerId !== agent.id),
            ...installations.map(installation => ({ installation, pending: installation.status === "ready" && !withoutQuota.has(installation.providerId) }))]);
          setPendingAgents(previous => previous.filter(id => id !== agent.id));
          await Promise.all(installations.filter(i => i.status === "ready" && !withoutQuota.has(i.providerId)).map(async installation => {
            let view: QuotaSnapshotView | undefined;
            let failed = false;
            try {
              const found = (await awaitDetection(client.quotaSnapshots(null, installation.installationId), controller.signal, quotaRequestTimeoutMs(installation.providerId))).find(v => v.installationId === installation.installationId && v.providerId === installation.providerId);
              view = found && withLastReading(found);
            }
            catch { failed = true; }
            const observedAt = Date.now();
            if (current()) setReadings(previous => previous.map(r => r.installation.installationId === installation.installationId ? { ...r, view, failed, observedAt, pending: false } : r));
          }));
        } catch (cause) {
          if (current()) {
            setFailureDetails(previous => ({ ...previous, [agent.id]: userFacingError(cause, "暂时无法确认安装，请重新检测。") }));
            setFailedAgents(previous => [...previous, agent.id]);
            setPendingAgents(previous => previous.filter(id => id !== agent.id));
            setError("部分 Agent 检测失败，可重试。若有正在运行的对话，请等待其完成；其余卡片不受影响。");
          }
        }
      }));
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
    const scanning = pendingAgents.includes(agent.id);
    const failed = failedAgents.includes(agent.id);
    const uninstalled = !reading && !scanning && !failed;
    const state = scanning ? { label: "检测中", tone: "muted", detail: "正在检测安装、登录与连接状态…" }
      : failed ? { label: "检测失败", tone: "warning", detail: "本轮检测未完成，请检查原生客户端或网络后重试。" } : welcomeStatus(reading);
    return { agent, reading, scanning, failed, uninstalled, state };
  });
  const connected = cards.filter(card => !card.uninstalled);
  const available = cards.filter(card => card.uninstalled);
  const readyCount = cards.filter(card => card.reading && card.state.tone === "ready").length;
  const attentionCount = cards.filter(card => card.failed || (card.reading && ["warning", "third-party"].includes(card.state.tone))).length;
  const scanningCount = pendingAgents.length;
  const logo = new URL("../src-tauri/icons/128x128.png", import.meta.url).href;
  const rescan = <button className="welcome-rescan" type="button" disabled={refreshing} onClick={() => void run()}>
    {refreshing ? <><span className="welcome-spinner" aria-hidden="true" />正在检测…</> : <><RefreshIcon />重新检测</>}
  </button>;

  const renderCard = ({ agent, reading, scanning, failed, uninstalled, state }: typeof cards[number], index: number) => {
    if (uninstalled) return <article className="welcome-agent-card uninstalled" key={agent.id} aria-label={agent.name} style={{ "--stagger": index } as CSSProperties}>
      <header><span className="welcome-agent-icon">{renderIcon(agent.id)}</span><div><h2>{agent.name}</h2><small>未安装</small></div></header>
      <InstallationLink url={agent.installUrl} name={agent.name} />
    </article>;
    const quota = reading?.view?.quota;
    const displayWindows = (quota?.windows ?? []).filter(w => agent.id !== "antigravity"
      || ((w.scope === "Gemini Models" || w.id.startsWith("Gemini Models:"))
        && (w.windowMinutes === 300 || w.windowMinutes === 10_080)));
    const groups = [...new Set(displayWindows.map(w => w.scope ?? ""))];
    // A reading that could not be refreshed is shown greyed with its time, never as current.
    const outdated = !scanning && displayWindows.length > 0 && quotaShowsLastReading(reading?.view?.diagnostic);
    const busy = scanning || state.label === "验证授权中";
    const emptyMessage = scanning || reading?.pending ? "正在等待检测结果" : failed ? "检测未完成，请重新检测"
      : withoutQuota.has(agent.id) ? "暂未接入账户额度查询" : "暂无可用额度读数";
    return <article className={`welcome-agent-card ${state.tone}${busy ? " busy" : ""}`} key={agent.id} aria-label={agent.name} style={{ "--stagger": index } as CSSProperties}>
      <header>
        <span className="welcome-agent-icon">{renderIcon(agent.id)}</span>
        <div><h2>{agent.name}</h2><small>{reading ? "已安装" : scanning ? "正在寻找本机客户端" : "安装状态待确认"}{outdated && quota && ` · 上次读数 ${new Date(quota.collectedAtUnixMs).toLocaleString([], { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" })}`}</small></div>
        <span className={`welcome-status ${state.tone}`} data-tooltip={failed ? failureDetails[agent.id] : state.detail} aria-busy={busy}>
          {busy ? <span className="welcome-spinner" aria-hidden="true" /> : <i aria-hidden="true" />}{state.label}
          {!scanning && !failed && reading && needsAgentLogin(reading.installation, reading.view) &&
            <AgentLogin key={reading.installation.installationId} installationId={reading.installation.installationId} name={agent.name} />}
        </span>
      </header>
      <div className={`welcome-card-quotas${outdated ? " outdated" : ""}`}>
        {!scanning && displayWindows.length ? groups.map(group => <section key={group} aria-label={quotaScopeLabel(group) || "账户额度"}>
          <div className="welcome-rings">{displayWindows.filter(w => (w.scope ?? "") === group)
            .map(w => <WindowRing key={w.id} window={w} quota={quota} scope={agent.id !== "antigravity" ? quotaScopeLabel(group) : ""} />)}</div>
        </section>) : <div className={`welcome-empty-quota${busy ? " loading" : ""}`}>
          {busy && <span className="welcome-skeleton" aria-hidden="true"><i /><i /></span>}
          <p>{emptyMessage}</p>
        </div>}
      </div>
      {reading && <footer>
        <details><summary>安装位置</summary><code>{reading.installation.executable}</code></details>
      </footer>}
    </article>;
  };

  return <main className="agent-welcome">
    <header className="welcome-topbar" data-tauri-drag-region="deep">
      <span className="welcome-brand"><img src={logo} alt="" />dog.codes</span>
      <button className="welcome-skip" type="button" onClick={onContinue}>跳过，进入工作台 <span aria-hidden="true">→</span></button>
    </header>
    <div className="welcome-page-body">
      <section className="welcome-hero">
        <div className="welcome-hero-copy">
          <p className="welcome-eyebrow"><span aria-hidden="true" />欢迎使用 dog.codes</p>
          <h1>把你的 Coding Agent<br />召集到一处</h1>
          <p className="welcome-lede">自动发现本机已安装的 Agent，统一查看登录状态与剩余额度。一个工作台，随时切换、协同完成任务。</p>
          <dl className="welcome-stats" aria-live="polite">
            <div className="ready"><dt>已就绪</dt><dd>{readyCount}</dd></div>
            <div className="attention"><dt>需处理</dt><dd>{attentionCount}</dd></div>
            <div><dt>{scanningCount ? "检测中" : "可添加"}</dt><dd>{scanningCount || available.length}</dd></div>
          </dl>
          <div className="welcome-actions">
            <button className="welcome-primary" type="button" onClick={onContinue}>开始使用 <span aria-hidden="true">→</span></button>
            {rescan}
          </div>
        </div>
        <WelcomeHerd renderIcon={renderIcon} agents={supportedAgents.map(agent => {
          const card = cards.find(item => item.agent.id === agent.id)!;
          return { id: agent.id, tone: card.reading ? "installed" : card.scanning ? "scanning" : card.failed ? "failed" : "missing" };
        })} />
      </section>

      {error && <p className="welcome-error" role="alert">{error}</p>}

      <section className="welcome-section" aria-labelledby="welcome-connected-title">
        <header className="welcome-section-heading">
          <div><h2 id="welcome-connected-title">你的 Agent</h2><p>{refreshing ? "正在检查本机连接…" : `已发现 ${installedAgents.size} 个本机 Agent`}</p></div>
        </header>
        {connected.length ? <div className="welcome-agent-grid">{connected.map(renderCard)}</div>
          : <div className="welcome-none"><b>还没有发现本机 Agent</b><p>从下方安装任意一个 Coding Agent，完成后点击“重新检测”。</p></div>}
      </section>

      {available.length > 0 && <section className="welcome-section" aria-labelledby="welcome-available-title">
        <header className="welcome-section-heading">
          <div><h2 id="welcome-available-title">更多可连接的 Agent</h2><p>安装后重新检测，即可在工作台中直接使用。</p></div>
        </header>
        <div className="welcome-available-grid">{available.map(renderCard)}</div>
      </section>}

      <footer className="welcome-bottom">
        <p>之后也可以在侧边栏“连接 Agent”中回到这里。</p>
        <button className="welcome-primary" type="button" onClick={onContinue}>开始使用 <span aria-hidden="true">→</span></button>
      </footer>
    </div>
  </main>;
}

function RefreshIcon() {
  return <svg className="welcome-button-icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3" /></svg>;
}
