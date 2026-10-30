import type { CoreClient, QuotaSnapshotView, TaskId } from "@dogcodes/protocol";
import { visibleAgentInstallations } from "./agentOptions";
import { withLastReading } from "./quotaFallback";

export type QuotaRefreshProgress = { running: boolean; completed: number; total: number; unavailable: number; failed: boolean };

export const quotaRequestTimeoutMs = (providerId?: string): number => providerId === "antigravity" ? 55_000 : 35_000;

function bounded<T>(request: Promise<T>, timeoutMs = 35_000): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("Quota request timed out")), timeoutMs);
    request.then(resolve, reject).finally(() => clearTimeout(timer));
  });
}

/** Publish each provider immediately; manual refresh waits for every request. */
export function createQuotaRefresh(
  client: CoreClient, taskId: TaskId | null, update: (view: QuotaSnapshotView) => void,
  progress: (state: QuotaRefreshProgress) => void = () => {},
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
      const state: QuotaRefreshProgress = { running: true, completed: 0, total: 0, unavailable: 0, failed: false };
      progress({ ...state });
      manual = (async () => {
        try {
          const snapshot = await bounded(client.bootstrap());
          if (!active) return;
          const installations = [...new Map(visibleAgentInstallations(snapshot.installations).map(i => [i.installationId, i])).values()];
          const scans = new Map<string, ReturnType<CoreClient["scanAgent"]>>();
          state.total = installations.length;
          progress({ ...state });
          await Promise.all(installations.map(async installation => {
            let current = installation;
            if (installation.status === "not_configured" || installation.status === "installed") {
              try {
                // External CLI/app login does not register a driver. Refresh readiness first;
                // the backend scan also refuses replacement while a conversation is running.
                let scan = scans.get(installation.providerId);
                if (!scan) {
                  scan = bounded(client.scanAgent(installation.providerId), 100_000);
                  scans.set(installation.providerId, scan);
                }
                const detected = visibleAgentInstallations((await scan).installations)
                  .filter(i => i.providerId === installation.providerId);
                current = detected.find(i => i.installationId === installation.installationId)
                  ?? (detected.length === 1 ? detected[0] : installation);
              } catch { /* Other providers still refresh; the existing reading may remain usable. */ }
            }
            const available = active && await read(current.installationId, true, current.providerId);
            state.completed++;
            if (!available) state.unavailable++;
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
