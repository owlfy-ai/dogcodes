// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CommandReceipt, CoreClient, TaskSummary } from "@dogcodes/protocol";
import { CancelRecovery } from "./CancelRecovery";
import { cancelOperations as operations } from "./cancelOperations";
const task: TaskSummary = { taskId: "tsk_test", projectId: "prj_test", title: "Task", streamVersion: 7, preferredExecutor: { providerId: "codex", installationId: null },
  activePrimaryRun: { runId: "run_old", turnId: "trn_old", executor: { providerId: "codex", installationId: null }, state: "running", startedAt: "2026-10-16T00:00:00Z" } };
const ack: CommandReceipt = { commandId: "cmd_old", accepted: true, firstSequence: 20, lastSequence: 20, requestFingerprint: "v1:hash" };
let host: HTMLDivElement, root: Root;
beforeEach(() => { localStorage.clear(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
function fixture() {
  const cancelTurn = vi.fn<CoreClient["cancelTurn"]>().mockRejectedValue(new Error("offline"));
  const cancelTurnReceipt = vi.fn<NonNullable<CoreClient["cancelTurnReceipt"]>>().mockResolvedValue(null);
  const client = { cancelTurn, cancelTurnReceipt } as unknown as CoreClient;
  const onAccepted = vi.fn();
  const render = (options: { taskId?: TaskSummary["taskId"]; busy?: boolean; activeRunId?: string | null; revision?: number } = {}) => act(async () => root.render(
    <CancelRecovery client={client} taskId={options.taskId ?? task.taskId} busy={options.busy ?? false}
      activeRunId={"activeRunId" in options ? options.activeRunId : "run_old"} revision={options.revision ?? 0} onAccepted={onAccepted}/>));
  return { client, cancelTurn, cancelTurnReceipt, onAccepted, render };
}
it("shows nothing, never resends, and settles once the receipt appears", async () => {
  const f = fixture(); await expect(operations.start(f.client, task)).rejects.toThrow();
  await f.render();
  expect(host.textContent).toBe(""); expect(f.cancelTurn).toHaveBeenCalledTimes(1); expect(operations.read(task.taskId)).toHaveLength(1);
  f.cancelTurnReceipt.mockResolvedValue(ack); await f.render({ revision: 1 });
  expect(operations.read(task.taskId)).toEqual([]); expect(f.onAccepted).toHaveBeenCalledOnce(); expect(f.cancelTurn).toHaveBeenCalledTimes(1);
});
it("drops a stop whose run is over and waits while the first send is in flight", async () => {
  const f = fixture(); await expect(operations.start(f.client, task)).rejects.toThrow();
  await f.render({ busy: true }); expect(f.cancelTurnReceipt).toHaveBeenCalledTimes(1);
  await f.render({ activeRunId: undefined }); expect(operations.read(task.taskId)).toHaveLength(1);
  await f.render({ activeRunId: null }); expect(operations.read(task.taskId)).toEqual([]);
  expect(f.onAccepted).not.toHaveBeenCalled(); expect(f.cancelTurn).toHaveBeenCalledTimes(1);
});
it("does not report to a different task when an old lookup finishes", async () => {
  const f = fixture(); await expect(operations.start(f.client, task)).rejects.toThrow();
  let resolve!: (value: CommandReceipt) => void;
  f.cancelTurnReceipt.mockReturnValueOnce(new Promise(done => { resolve = done; }));
  await f.render(); await f.render({ taskId: "tsk_other" });
  await act(async () => resolve(ack));
  expect(f.onAccepted).not.toHaveBeenCalled(); expect(host.textContent).toBe("");
  expect(operations.read(task.taskId)).toEqual([]);
});
