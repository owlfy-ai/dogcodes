// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CommandReceipt, CoreClient, TaskSummary } from "@dogcodes/protocol";
import { CancelRecovery } from "./CancelRecovery";
import { cancelOperations as operations } from "./cancelOperations";
const task: TaskSummary = { taskId: "tsk_test", projectId: "prj_test", title: "Task", streamVersion: 7, preferredExecutor: { providerId: "codex", installationId: null },
  activePrimaryRun: { runId: "run_old", turnId: "trn_old", executor: { providerId: "codex", installationId: null }, state: "running", startedAt: "2026-10-16T00:00:00Z" } };
let host: HTMLDivElement, root: Root;
const button = (text: string) => [...host.querySelectorAll("button")].find(node => node.textContent === text)!;
const click = async (text: string) => act(async () => button(text).click());
beforeEach(() => { localStorage.clear(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
function fixture() {
  const cancelTurn = vi.fn<CoreClient["cancelTurn"]>().mockRejectedValue(new Error("offline"));
  const cancelTurnReceipt = vi.fn<NonNullable<CoreClient["cancelTurnReceipt"]>>().mockResolvedValue(null);
  const client = { cancelTurn, cancelTurnReceipt } as unknown as CoreClient;
  const onAccepted = vi.fn();
  const render = (taskId = task.taskId, busy = false) => act(async () => root.render(<CancelRecovery client={client} taskId={taskId} busy={busy} onAccepted={onAccepted}/>));
  return { client, cancelTurn, cancelTurnReceipt, onAccepted, render };
}
it("restores without dispatch, checks read-only, and explicitly resends only the original stop", async () => {
  const f = fixture(); await expect(operations.start(f.client, task)).rejects.toThrow();
  const record = operations.read(task.taskId)[0];
  await f.render(); await act(async () => root.unmount()); root = createRoot(host); await f.render();
  expect(f.cancelTurn).toHaveBeenCalledTimes(1); expect(f.cancelTurnReceipt).toHaveBeenCalledTimes(1);
  await click("核实停止请求"); expect(f.cancelTurn).toHaveBeenCalledTimes(1);
  f.cancelTurn.mockResolvedValue(); await click("重发原停止请求");
  expect(f.cancelTurn.mock.calls[1]).toEqual([task, record.operationId]);
  expect(host.querySelector('[aria-label="待核实的停止请求"]')).toBeNull();
  expect(f.onAccepted).toHaveBeenCalledOnce();
  expect(host.textContent).not.toContain("任务已停止");
});
it("disables recovery while the initial operation is still in flight and explains acceptance", async () => {
  const f = fixture(); await expect(operations.start(f.client, task)).rejects.toThrow(); await f.render(task.taskId, true);
  expect(button("核实停止请求").disabled).toBe(true);
  expect(host.textContent).toContain("仍需等待任务实际退出");
  expect(host.textContent).toContain("不会撤回已发送的停止请求");
});
it("does not update a different task when an old lookup finishes", async () => {
  const f = fixture(); await expect(operations.start(f.client, task)).rejects.toThrow(); await f.render();
  let resolve!: (value: CommandReceipt) => void;
  f.cancelTurnReceipt.mockReturnValueOnce(new Promise(done => { resolve = done; }));
  await click("核实停止请求"); await f.render("tsk_other");
  await act(async () => resolve({ commandId: "cmd_old", accepted: true, firstSequence: 20, lastSequence: 20, requestFingerprint: "v1:hash" }));
  expect(f.onAccepted).not.toHaveBeenCalled(); expect(host.textContent).toBe("");
  expect(operations.read(task.taskId)).toEqual([]);
});
