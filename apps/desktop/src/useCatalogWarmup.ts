import { useEffect } from "react";
import type { CoreClient, InstallationSummary } from "@dogcodes/protocol";
import { ModelCatalogCache } from "./modelCatalogCache";
import { invalidatePermissionCatalogs, permissionCatalogEntry } from "./permissionCatalogCache";
export function useCatalogWarmup(client: CoreClient, models: ModelCatalogCache, installations: InstallationSummary[], readyIds: string[]) {
  const scopes = JSON.stringify(installations.filter(i => i.providerId !== "fake" && readyIds.includes(`${i.providerId}:${i.installationId}`))
    .map(i => ({ executor: { providerId: i.providerId, installationId: i.installationId }, version: i.version ?? null })));
  useEffect(() => {
    let active = true, checking = false;
    let revision: number | undefined;
    const targets = JSON.parse(scopes) as Array<{ executor: { providerId: string; installationId: string }; version: string | null }>;
    const warm = () => {
      for (const { executor, version } of targets) {
        void models.warm(executor, version).catch(() => {});
        void permissionCatalogEntry(client, executor, undefined, version).get().catch(() => {});
      }
    };
    const check = async () => {
      if (!active || checking) return;
      checking = true;
      try {
        const next = await client.catalogRevision?.();
        if (!active) return;
        if (next !== undefined && revision !== undefined && next !== revision) {
          models.invalidate(); invalidatePermissionCatalogs(client);
        }
        revision = next;
        warm();
      } catch { /* Keep successful values while Core reconnects. */ }
      finally { checking = false; }
    };
    void check();
    const timer = window.setInterval(() => void check(), 5000);
    return () => { active = false; window.clearInterval(timer); };
  }, [client, models, scopes]);
}
