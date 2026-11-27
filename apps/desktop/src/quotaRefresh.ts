import type { CoreClient, InstallationSummary, QuotaSnapshotView, TaskId } from "@dogcodes/protocol";
import { installationsOnTarget, visibleAgentInstallations } from "./agentOptions";
import { withLastReading } from "./quotaFallback";

export type AgentRefreshProgress = { running: boolean; completed: number; total: number; unavailable: number; failed: boolean };

export const quotaRequestTimeoutMs = (providerId?: string): number => providerId === "antigravity" ? 55_000 : 35_000;

function bounded<T>(request: Promise<T>, timeoutMs = 35_000): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Agent request timed out")), timeoutMs);
    request.then(resolve, reject).finally(() => clearTimeout(timer));
  });
}

/** Publish each provider immediately; manual refresh waits for every request. */
export function createQuotaRefresh(
  client: CoreClient, taskId: TaskId | null, update: (view: QuotaSnapshotView) => void,
  progress: (state: AgentRefreshProgress) => void = () => {},
  refreshDetails: (installation: InstallationSummary) => Promise<void> = async () => {},
) {
  let active = true;
  let manual: Promise<void> | undefined;
  const pending = new Map<string, Promise<boolean>>();
  const read = (id: string, force: boolean, providerId?: string): Promise<boolean> => {
    const previous = pending.get(id);
    if (previous && !force) return previous;
    const request = (async () => {
      // Finish an automatic read before issuing the explicitly fresh request.
      if (previous) await previous;
      if (!active) return false;
      try {
        const views = await bounded(force ? client.quotaSnapshots(taskId, id, true) : client.quotaSnapshots(taskId, id), quotaRequestTimeoutMs(providerId));
        const matching = views.filter(view => view.installationId === id);
        if (active) for (const view of matching) update(withLastReading(view));
        return matching.some(view => (!!view.quota?.windows.length || !!view.quota?.balances?.length) && !view.diagnostic);
      } catch { return false; } // Preserve the last reading and its original timestamp.
    })();
    pending.set(id, request);
    void request.finally(() => { if (pending.get(id) === request) pending.delete(id); });
    return request;
  };
  return {
    refreshInstallation(id: string, providerId: string) { return active ? read(id, false, providerId) : Promise.resolve(false); },
    async refresh() {
      if (manual || !active) return;
      const snapshot = await bounded(client.bootstrap());
      if (!active || manual) return;
      for (const installation of visibleAgentInstallations(snapshot.installations)) void read(installation.installationId, false, installation.providerId);
    },
    refreshNow(): Promise<void> {
      if (manual) return manual;
      if (!active) return Promise.resolve();
      const state: AgentRefreshProgress = { running: true, completed: 0, total: 0, unavailable: 0, failed: false };
      progress({ ...state });
      manual = (async () => {
        try {
          const snapshot = await bounded(client.bootstrap());
          if (!active) return;
          // The agent page is this machine's. A server's has its own, and scanning below
          // would only look for its agents here.
          const installations = [...new Map(visibleAgentInstallations(installationsOnTarget(snapshot.installations, null)).map(i => [i.installationId, i])).values()];
          const scans = new Map<string, ReturnType<CoreClient["scanAgent"]>>();
          state.total = installations.length;
          progress({ ...state });
          await Promise.all(installations.map(async installation => {
            let current: InstallationSummary | undefined = installation;
            let complete = true;
            try {
              let scan = scans.get(installation.providerId);
              if (!scan) {
                scan = bounded(client.scanAgent(installation.providerId), 100_000);
                scans.set(installation.providerId, scan);
              }
              const detected = installationsOnTarget((await scan).installations, null)
                .filter(i => i.providerId === installation.providerId);
              current = detected.find(i => i.installationId === installation.installationId)
                ?? (detected.length === 1 ? detected[0] : undefined);
            } catch {
              // A running conversation can prevent re-detection; its quota can still refresh.
              complete = false;
            }
            if (active && current) {
              const [quota, details] = await Promise.all([
                read(current.installationId, true, current.providerId),
                complete && bounded(refreshDetails(current), 100_000).then(() => true, () => false),
              ]);
              complete = quota && details;
            }
            state.completed++;
            if (!complete) state.unavailable++;
            if (active) progress({ ...state });
          }));
        } catch { state.failed = true; }
        finally {
          manual = undefined;
          state.running = false;
          if (active) progress({ ...state });
        }
      })();
      return manual;
    },
    dispose() { active = false; },
  };
}
