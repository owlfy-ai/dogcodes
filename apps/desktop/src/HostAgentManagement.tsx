import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { AgentUpdate, QuotaSnapshotView } from "@dogcodes/protocol";
import { QuotaPanel, type HostAgentActions } from "./QuotaPanel";
import type { AgentUpdateActions } from "./agentUpdates";
import type { HostAgentInstall, TargetApi, TargetEntry } from "./executionTargetClient";
import { loginCommand, runsOnServers } from "./hostAgentCommands";
import { useI18n } from "./i18n";
import { withLastReading } from "./quotaFallback";
import { userFacingError } from "./userFacingError";

const QUOTA_INTERVAL = 30_000;
const UPGRADE_FOLLOW_INTERVAL = 1500;
const INSTALL_FOLLOW_INTERVAL = 1500;
const SCAN_FOLLOW_INTERVAL = 500;
const SCAN_LIMIT = 120_000;

const checking = (id: string): AgentUpdate => ({ installationId: id, status: "checking", currentVersion: null, latestVersion: null, source: "", message: null, canUpgrade: false, checkedAt: 0 });

/** The same actions as this machine's agents, answered by the server's shim. */
function useHostAgentUpdates(api: TargetApi, targetId: string, installationIds: string[], connected: boolean): AgentUpdateActions {
  const [updates, setUpdates] = useState<Record<string, AgentUpdate>>({});
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const accept = useCallback((value: AgentUpdate) => { if (alive.current) setUpdates(old => ({ ...old, [value.installationId]: value })); }, []);
  const check = useCallback((id: string, force = false) => {
    setUpdates(old => old[id]?.status === "upgrading" ? old : { ...old, [id]: { ...checking(id), currentVersion: old[id]?.currentVersion ?? null } });
    void api.agentUpdate(targetId, id, force).then(accept).catch(() => accept({ ...checking(id), status: "error", message: "agentUi.updateCheckFailed" }));
  }, [api, targetId, accept]);
  const key = installationIds.join("|");
  useEffect(() => { if (connected) for (const id of installationIds) check(id); }, [key, connected, check]);
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
function useHostAgentInstalls(api: TargetApi, targetId: string, connected: boolean) {
  const [installs, setInstalls] = useState<HostAgentInstall[]>([]);
  const running = installs.some(install => install.status === "installing");
  useEffect(() => {
    if (!connected) return;
    let alive = true;
    const read = () => void api.agentInstalls(targetId).then(found => { if (alive) setInstalls(found); }).catch(() => {});
    read();
    const timer = running ? setInterval(read, INSTALL_FOLLOW_INTERVAL) : undefined;
    return () => { alive = false; clearInterval(timer); };
  }, [api, targetId, connected, running]);
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
export function HostAgentManagement({ api, entry, visible, offlineBanner, onRunInShell }: {
  api: TargetApi; entry: TargetEntry; visible: boolean; onRunInShell: (command: string) => void;
  /** Connects the host and says why not, the same as a project on it would. */
  offlineBanner: ReactNode;
}) {
  const { t } = useI18n();
  const { targetId } = entry.target;
  const connected = entry.status.phase === "connected";
  const [views, setViews] = useState<QuotaSnapshotView[]>([]);
  const [refreshing, setRefreshing] = useState(false);
  const [failure, setFailure] = useState("");
  const ids = entry.agents.map(agent => agent.installationId);
  const idsKey = ids.join("|");
  const updates = useHostAgentUpdates(api, targetId, ids, connected);
  const { installs, install } = useHostAgentInstalls(api, targetId, connected);
  const read = useCallback(async (force: boolean) => {
    await Promise.all(idsKey.split("|").filter(Boolean).map(async id => {
      try {
        const found = (await api.quota(id, force)).filter(view => view.installationId === id).map(withLastReading);
        setViews(old => [...old.filter(view => view.installationId !== id), ...found]);
      } catch { /* The last reading stays, with its original time. */ }
    }));
  }, [api, idsKey]);
  useEffect(() => {
    if (!visible || !connected) return;
    void read(false);
    const timer = setInterval(() => void read(false), QUOTA_INTERVAL);
    return () => clearInterval(timer);
  }, [visible, connected, read]);
  const refresh = () => {
    setRefreshing(true);
    void read(true).finally(() => setRefreshing(false));
    for (const id of ids) updates.check(id, true);
  };
  const latestEntry = useRef(entry);
  latestEntry.current = entry;
  const detect = async () => {
    setFailure("");
    try {
      await api.rescan(targetId);
      // It answers at once; the scan is over when the service says so.
      for (const started = Date.now(); Date.now() - started < SCAN_LIMIT;) {
        await new Promise(resolve => setTimeout(resolve, SCAN_FOLLOW_INTERVAL));
        if (!(await api.status(targetId)).scanning) return;
      }
    } catch (cause) {
      setFailure(userFacingError(cause, t("executionTargets.actionFailed")));
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
          <button className="quota-refresh-button" type="button" disabled={refreshing || !connected} onClick={refresh} aria-busy={refreshing}>
            {refreshing ? <span className="task-running-spinner" aria-hidden="true" /> : <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-1l2 6M4 12l2 6a7 7 0 0 0 12-1" /></svg>}
            {t(refreshing ? "agentManagement.refreshing" : "agentManagement.refreshQuota")}
          </button>
        </div>
        {failure && <div className="remote-error" role="alert">{failure}</div>}
        <div className="agent-management-summary"><span>{t("agentManagement.installedCount", { count: entry.agents.length })}</span><span>{t("agentManagement.quotaCalculation")}</span></div>
        <QuotaPanel views={views} installations={entry.agents} agentUpdates={updates} onDetect={detect} management host={host} />
      </div>
    </div>
  </div>;
}
