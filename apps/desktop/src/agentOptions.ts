import type { ExecutorRef, InstallationSummary } from "@dogcodes/protocol";

/** The backend resolves the user's command and returns one installation per Agent. */
export function visibleAgentInstallations(installations: InstallationSummary[]): InstallationSummary[] {
  return installations.filter(installation => installation.providerId !== "fake");
}

export function readyAgentInstallations(installations: InstallationSummary[]): InstallationSummary[] {
  return visibleAgentInstallations(installations).filter(installation => installation.status === "ready");
}

/** Restore a selection against discovery's current executable for the same Agent. */
export function currentAgentExecutor(executor: ExecutorRef, installations: InstallationSummary[]): ExecutorRef {
  const candidates = readyAgentInstallations(installations).filter(installation => installation.providerId === executor.providerId);
  if (candidates.some(installation => installation.installationId === executor.installationId)) return executor;
  // Do not guess when discovery is incomplete or the host exposes multiple installations.
  if (candidates.length !== 1) return executor;
  return { providerId: executor.providerId, installationId: candidates[0].installationId };
}

export function reviewAgentExecutors(installations: InstallationSummary[], connected: ExecutorRef[]): ExecutorRef[] {
  return readyAgentInstallations(installations).filter(installation => connected.some(executor =>
    executor.providerId === installation.providerId && executor.installationId === installation.installationId,
  )).map(installation => ({ providerId: installation.providerId, installationId: installation.installationId }));
}

/** Version comparison is only for update notices, never installation selection. */
export function compareVersions(left: string | null, right: string | null): number {
  const parts = (value: string | null) => value?.match(/\d+/g)?.map(Number) ?? [];
  const a = parts(left), b = parts(right);
  for (let index = 0; index < Math.max(a.length, b.length); index++) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference) return difference;
  }
  return 0;
}
