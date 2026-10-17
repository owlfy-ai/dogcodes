import { expect, it } from "vitest";
import type { InstallationSummary } from "@dogcodes/protocol";
import { currentAgentExecutor, visibleAgentInstallations, readyAgentInstallations, reviewAgentExecutors } from "./agentOptions";
const installation = (providerId: string, installationId: string, status = "ready") => ({ providerId, installationId, status }) as InstallationSummary;
it("uses the backend-selected installations and hides the internal driver", () => {
  const selected = [installation("claude-code", "ins_terminal"), installation("codex", "ins_codex", "broken"), installation("fake", "ins_fake")];
  expect(visibleAgentInstallations(selected).map(i => i.installationId)).toEqual(["ins_terminal", "ins_codex"]);
  expect(readyAgentInstallations(selected).map(i => i.installationId)).toEqual(["ins_terminal"]);
});
it("review cannot select an old connected executor excluded by command resolution", () => {
  const selected = [installation("claude-code", "ins_terminal")];
  const connected = [{ providerId: "claude-code", installationId: "ins_old" }, { providerId: "claude-code", installationId: "ins_terminal" }];
  expect(reviewAgentExecutors(selected, connected)).toEqual([connected[1]]);
  expect(reviewAgentExecutors(selected, connected.slice(0, 1))).toEqual([]);
});
it("reconciles replaced or missing installation IDs without changing Agents", () => {
  const current = installation("cursor", "ins_current");
  for (const installationId of [null, "ins_old"]) {
    expect(currentAgentExecutor({ providerId: "cursor", installationId }, [current]))
      .toEqual({ providerId: "cursor", installationId: "ins_current" });
  }
  const selected = { providerId: "cursor", installationId: "ins_current" };
  expect(currentAgentExecutor(selected, [current])).toBe(selected);
  expect(currentAgentExecutor(selected, [installation("codex", "ins_codex")])).toBe(selected);
  expect(currentAgentExecutor(selected, [installation("cursor", "ins_new", "broken")])).toBe(selected);
  expect(currentAgentExecutor(selected, [])).toBe(selected);
  expect(currentAgentExecutor(selected, [installation("cursor", "ins_a"), installation("cursor", "ins_b")])).toBe(selected);
});
