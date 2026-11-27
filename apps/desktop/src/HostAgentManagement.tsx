import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AgentUpdate, QuotaSnapshotView } from "@dogcodes/protocol";
import { QuotaCollectedTime, QuotaPanel, type HostAgentActions } from "./QuotaPanel";
import type { AgentUpdateActions } from "./agentUpdates";
import type { HostAgent, HostAgentInstall, TargetApi, TargetEntry } from "./executionTargetClient";
import { loginCommand, runsOnServers } from "./hostAgentCommands";
import { useI18n } from "./i18n";
import { withLastReading } from "./quotaFallback";
import { userFacingError } from "./userFacingError";

const QUOTA_INTERVAL = 30_000;
const UPGRADE_FOLLOW_INTERVAL = 1500;
const INSTALL_FOLLOW_INTERVAL = 1500;
const SCAN_FOLLOW_INTERVAL = 500;
const SCAN_LIMIT = 120_000;

// A file whose command cannot run is not a successful installation.
const isInstalled = (agent: HostAgent) => agent.status !== "broken" && agent.status !== "unsupported_version";
const checking = (id: string): AgentUpdate => ({ installationId: id, status: "checking", currentVersion: null, latestVersion: null, source: "", message: null, canUpgrade: false, checkedAt: 0 });

/** The same actions as this machine's agents, answered by the server's shim. */
function useHostAgentUpdates(api: TargetApi, targetId: string, installationIds: string[], connected: boolean): AgentUpdateActions {
  const [updates, setUpdates] = useState<Record<string, AgentUpdate>>({});
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const accept = useCallback((value: AgentUpdate) => { if (alive.current) setUpdates(old => ({ ...old, [value.installationId]: value })); }, []);
  const check = useCallback(async (id: string, force = false) => {
    setUpdates(old => old[id]?.status === "upgrading" ? old : { ...old, [id]: { ...checking(id), currentVersion: old[id]?.currentVersion ?? null } });
    try { accept(await api.agentUpdate(targetId, id, force)); }
    catch { accept({ ...checking(id), status: "error", message: "agentUi.updateCheckFailed" }); }
  }, [api, targetId, accept]);
  const key = installationIds.join("|");
  useEffect(() => { if (connected) for (const id of installationIds) void check(id); }, [key, connected, check]);
  const upgrading = Object.values(updates).filter(update => update.status === "upgrading").map(update => update.installationId).join("|");
  useEffect(() => {
    if (!upgrading) return;
    const timer = setInterval(() => {
      for (const id of upgrading.split("|")) void api.agentUpdate(targetId, id, false).then(accept).catch(() => {});
    }, UPGRADE_FOLLOW_INTERVAL);
    return () => clearInterval(timer);
  }, [upgrading, api, targetId, accept]);
  const upgrade = useCallback((id: string) => {
    setUpdates(old => ({ ...old, [id]: { ...(old[id] ?? checking(id)), status: "upgrading", canUpgrade: false, message: "agentUi.checkingUpgrade" } }));
    void api.upgradeAgent(targetId, id).then(accept).catch(cause => accept({ ...(updates[id] ?? checking(id)), status: "error", canUpgrade: Boolean(updates[id]?.latestVersion), message: typeof cause === "string" ? cause : "agentUi.upgradeFailed" }));
  }, [api, targetId, accept, updates]);
  return { updates, check, upgrade };
}

/** Installs go on without this page, so it picks up whatever is under way. */
function useHostAgentInstalls(api: TargetApi, targetId: string, connected: boolean, scanKey: string) {
  const [installs, setInstalls] = useState<HostAgentInstall[]>([]);
  const running = installs.some(install => install.status === "installing");
  useEffect(() => {
    let alive = true;
    const read = () => void api.agentInstalls(targetId).then(found => { if (alive) setInstalls(found); }).catch(() => {});
    read();
    const timer = connected && running ? setInterval(read, INSTALL_FOLLOW_INTERVAL) : undefined;
    return () => { alive = false; clearInterval(timer); };
  }, [api, targetId, connected, running, scanKey]);
  const install = useCallback((providerId: string) => {
    setInstalls(old => [...old.filter(item => item.providerId !== providerId), { providerId, status: "installing", message: null }]);
    void api.installAgent(targetId, providerId).then(setInstalls).catch((cause: unknown) => {
      setInstalls(old => [...old.filter(item => item.providerId !== providerId), { providerId, status: "failed", message: typeof cause === "string" ? cause : null }]);
    });
  }, [api, targetId]);
  return { installs, install };
}

/** One server's agents, managed like this machine's: balance and reset times, installing,
 * signing in and upgrading. Installing is done by the server's shim. Signing in happens in a
 * shell tab on the server, where its prompts can be answered. */
export function HostAgentManagement({ api, entry, visible, offlineBanner, onRunInShell, onRefreshed }: {
  api: TargetApi; entry: TargetEntry; visible: boolean; onRunInShell: (command: string) => void;
  onRefreshed: (entry: TargetEntry) => void;
  /** Connects the host and says why not, the same as a project on it would. */
  offlineBanner: ReactNode;
}) {
  const { t } = useI18n();
  const { targetId } = entry.target;
  const connected = entry.status.phase === "connected";
  const [views, setViews] = useState<QuotaSnapshotView[]>([]);
  const [refreshing, setRefreshing] = useState(false);
  const [failure, setFailure] = useState("");
  const manualRefresh = useRef(false);
  const reads = useRef(new Map<string, symbol>());
  // Failed probes stay available through agentInstalls as retryable error rows.
  const installed = entry.agents.filter(isInstalled);
  const scanKey = entry.agents.map(agent => `${agent.installationId}:${agent.status}:${agent.diagnostic}`).join("|");
  const ids = installed.map(agent => agent.installationId);
  const idsKey = ids.join("|");
  const updates = useHostAgentUpdates(api, targetId, ids, connected);
  const { installs, install } = useHostAgentInstalls(api, targetId, connected, scanKey);
  const read = useCallback(async (ids: string[], force: boolean) => {
    await Promise.all(ids.map(async id => {
      const request = Symbol();
      reads.current.set(id, request);
      try {
        const response = await api.quota(id, force);
        if (reads.current.get(id) !== request) return;
        const found = response.filter(view => view.installationId === id).map(withLastReading);
        if (found.length) setViews(old => [...old.filter(view => view.installationId !== id), ...found]);
      } catch { /* The last reading stays, with its original time. */ }
      finally { if (reads.current.get(id) === request) reads.current.delete(id); }
    }));
  }, [api]);
  useEffect(() => {
    if (!visible || !connected) return;
    const ids = idsKey.split("|").filter(Boolean);
    const poll = () => { if (!manualRefresh.current) void read(ids, false); };
    poll();
    const timer = setInterval(poll, QUOTA_INTERVAL);
    return () => clearInterval(timer);
  }, [visible, connected, idsKey, read]);
  const latestEntry = useRef(entry);
  latestEntry.current = entry;
  const detect = async () => {
    setFailure("");
    try {
      await api.rescan(targetId);
      // The request starts a scan. Wait for it to publish the new installations.
      for (const started = Date.now(); Date.now() - started < SCAN_LIMIT;) {
        const status = await api.status(targetId);
        if (status.phase !== "connected") break;
        if (!status.scanning) {
          const next = (await api.list()).find(entry => entry.target.targetId === targetId);
          if (!next) break;
          onRefreshed(next);
          return next;
        }
        await new Promise(resolve => setTimeout(resolve, SCAN_FOLLOW_INTERVAL));
      }
      setFailure(t("agentManagement.refreshFailed"));
    } catch (cause) {
      setFailure(userFacingError(cause, t("executionTargets.actionFailed")));
    }
  };
  const refresh = async () => {
    if (manualRefresh.current) return;
    manualRefresh.current = true;
    setRefreshing(true);
    try {
      const refreshed = await detect();
      const ids = (refreshed ?? latestEntry.current).agents.filter(isInstalled).map(agent => agent.installationId);
      await Promise.all([read(ids, true), ...ids.map(id => updates.check(id, true))]);
    } finally {
      manualRefresh.current = false;
      setRefreshing(false);
    }
  };
  const host = useMemo<HostAgentActions>(() => ({
    supports: runsOnServers,
    installs,
    install,
    login: (installationId, providerId) => {
      const agent = latestEntry.current.agents.find(item => item.installationId === installationId);
      if (agent) onRunInShell(loginCommand(providerId, agent.executable));
    },
  }), [onRunInShell, installs, install]);
  return <div className="target-agents-page" role="tabpanel" hidden={!visible}>
    {offlineBanner}
    <div className="quota-center agent-management" aria-label={t("executionTargets.agentsOf", { name: entry.target.displayName })}>
      <div className="quota-center-content">
        <div className="quota-center-intro">
          <p className="agent-management-description">{t("executionTargets.agentsPageHint", { name: entry.target.displayName })}</p>
          <button className="quota-refresh-button" type="button" disabled={refreshing || !connected} onClick={() => void refresh()} aria-busy={refreshing}>
            {refreshing ? <span className="task-running-spinner" aria-hidden="true" /> : <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-1l2 6M4 12l2 6a7 7 0 0 0 12-1" /></svg>}
            {t(refreshing ? "agentManagement.refreshing" : "agentManagement.refreshStatus")}
          </button>
        </div>
        {failure && <div className="remote-error" role="alert">{failure}</div>}
        <div className="agent-management-summary"><span>{t("agentManagement.installedCount", { count: installed.length })}</span><span>{t("agentManagement.quotaCalculation")}<QuotaCollectedTime views={views.filter(view => ids.some(id => id === view.installationId))} /></span></div>
        <QuotaPanel views={views} installations={installed} agentUpdates={updates} onDetect={async () => { await detect(); }} management host={host} />
      </div>
    </div>
  </div>;
}
