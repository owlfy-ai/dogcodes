import type { InstallationSummary, QuotaSnapshotView } from "@dogcodes/protocol";
import { readyAgentInstallations } from "./agentOptions";

/** Readings always belong to the selected installation, even if another has quota data. */
export function visibleQuotaViews(views: QuotaSnapshotView[], installations: Array<Pick<InstallationSummary, "installationId" | "version">>): QuotaSnapshotView[] {
  return installations.flatMap(installation => {
    const view = views.find(item => item.providerId !== "fake" && item.installationId === installation.installationId);
    return view ? [view] : [];
  });
}

export function quotaProviderCount(installations: InstallationSummary[]): number {
  return readyAgentInstallations(installations).length;
}
