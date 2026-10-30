import type { QuotaSnapshot, QuotaSnapshotView } from "@dogcodes/protocol";

// While quota reads are rate limited or Claude Code's token awaits renewal, keep
// showing the last successful reading. Readings are stored so they also survive an app restart; each keeps
// its own collection time, so the UI already presents it as stale.
const storageKey = "dogcodes.quota.lastReading.v1";
// Failures that leave the last reading outdated rather than wrong.
const outdatedBy = new Set(["quota_rate_limited", "claude_quota_rate_limited", "claude_quota_token_expired"]);

export const quotaShowsLastReading = (diagnostic?: string | null): boolean => !!diagnostic && outdatedBy.has(diagnostic);

function readings(): Record<string, QuotaSnapshot> {
  try { return JSON.parse(localStorage.getItem(storageKey) ?? "{}") ?? {}; } catch { return {}; }
}

/** Remember a successful reading, or stand in the last one while it cannot be refreshed. */
export function withLastReading(view: QuotaSnapshotView): QuotaSnapshotView {
  const quota = view.quota;
  // Provider API credentials can rotate without changing installationId. These readings
  // are cached with credential identity in Rust; never restore another account in the UI.
  if (["opencode", "pi", "deepseek-harness"].includes(view.providerId)) return view;
  if (quota?.windows.length && !view.diagnostic) {
    if (quota.confidence === "demo") return view;
    const stored = readings();
    if (stored[view.installationId]?.collectedAtUnixMs !== quota.collectedAtUnixMs) {
      // Session usage and context belong to one conversation, not to the account reading.
      stored[view.installationId] = { ...quota, sessionUsage: null, context: null };
      try { localStorage.setItem(storageKey, JSON.stringify(stored)); } catch { /* Storage can be disabled. */ }
    }
    return view;
  }
  if (!quotaShowsLastReading(view.diagnostic) || quota?.windows.length) return view;
  const last = readings()[view.installationId];
  if (!last?.windows?.length) return view;
  return { ...view, quota: { ...last, sessionUsage: quota?.sessionUsage ?? null, context: quota?.context ?? null } };
}
