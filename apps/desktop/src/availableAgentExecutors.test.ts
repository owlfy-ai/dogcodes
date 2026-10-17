import { expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { availableAgentExecutors } from "./availableAgentExecutors";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
it("excludes stale registered drivers and non-ready installations from run and commit menus", async () => {
  const current = { providerId: "codex", installationId: "ins_current" };
  vi.mocked(invoke).mockImplementation(async command => command === "core_bootstrap" ? { installations: [
    { ...current, status: "ready" }, { providerId: "kimi", installationId: "ins_kimi", status: "broken" },
  ] } : [current, { providerId: "codex", installationId: "ins_old" }, { providerId: "kimi", installationId: "ins_kimi" }, { providerId: "cursor", installationId: "ins_removed" }]);
  expect(await availableAgentExecutors()).toEqual([current]);
});
