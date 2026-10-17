import { expect, it } from "vitest";
import type { InstallationSummary, QuotaSnapshotView } from "@dogcodes/protocol";
import { visibleQuotaViews, quotaProviderCount } from "./quotaInstallations";
const selected: InstallationSummary[] = [{ providerId: "claude-code", installationId: "ins_terminal", status: "ready", version: "1.0", executable: "/bin/claude", fingerprint: "terminal", versionText: "1.0", diagnostic: null, probeElapsedMillis: 1, probedAt: "2026-09-21" }];
const view = (id: string, available = true) => ({ installationId: id, providerId: "claude-code", quota: available ? { windows: [] } : null }) as QuotaSnapshotView;
it("ignores readings from other installations regardless of arrival order", () => {
  for (const values of [[view("ins_old"), view("ins_terminal")], [view("ins_terminal"), view("ins_old")]]) {
    expect(visibleQuotaViews(values, selected).map(v => v.installationId)).toEqual(["ins_terminal"]);
  }
  expect(quotaProviderCount(selected)).toBe(1);
});
it("does not substitute another installation's quota when the selected one is unavailable", () => {
  expect(visibleQuotaViews([view("ins_old"), view("ins_terminal", false)], selected)[0].quota).toBeNull();
  expect(visibleQuotaViews([view("ins_old")], selected)).toEqual([]);
});
