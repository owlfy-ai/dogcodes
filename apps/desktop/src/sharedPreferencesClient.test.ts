import { expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { TauriCoreClient } from "./tauriClient";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: vi.fn() }));

it("sends version zero and a stable operation identity through the existing command envelope", async () => {
  const client = new TauriCoreClient();
  vi.mocked(invoke).mockResolvedValue(undefined);
  const preferences = { order: ["kimi"], disabled: ["codex"] };
  await client.setAgentPreferences(preferences, 0, "cmd_stable");
  await client.setAgentPreferences(preferences, 0, "cmd_stable");
  for (const [name, args] of vi.mocked(invoke).mock.calls) {
    expect(name).toBe("core_execute");
    expect(args).toMatchObject({ envelope: { commandId: "cmd_stable", idempotencyKey: "cmd_stable",
      expectedStreamVersion: 0, actor: { kind: "local_user" }, payload: { type: "set_agent_preferences", data: preferences } } });
  }
  await client.agentPreferences();
  expect(invoke).toHaveBeenLastCalledWith("core_agent_preferences");
});

it("uses the same versioned envelope for history organization and never sends layout fields", async () => {
  vi.mocked(invoke).mockClear();
  const client = new TauriCoreClient();
  const preferences = { tasks: { tsk_one: "archived" as const }, projects: {}, sorts: {}, orders: {} };
  await client.setHistoryPreferences(preferences, 0, "cmd_history");
  expect(invoke).toHaveBeenLastCalledWith("core_execute", expect.objectContaining({ envelope: expect.objectContaining({
    commandId: "cmd_history", idempotencyKey: "cmd_history", expectedStreamVersion: 0,
    payload: { type: "set_history_preferences", data: preferences },
  }) }));
  await client.historyPreferences();
  expect(invoke).toHaveBeenLastCalledWith("core_history_preferences");
});
