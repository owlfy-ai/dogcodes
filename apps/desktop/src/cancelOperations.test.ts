// @vitest-environment jsdom
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { CommandReceipt, CoreClient, TaskSummary } from "@dogcodes/protocol";
import { cancelOperations as operations } from "./cancelOperations";
const task: TaskSummary = { taskId: "tsk_original", projectId: "prj_test", title: "Task", streamVersion: 7, preferredExecutor: { providerId: "codex", installationId: null },
  activePrimaryRun: { runId: "run_original", turnId: "trn_original", executor: { providerId: "codex", installationId: null }, state: "running", startedAt: "2026-10-16T00:00:00Z" } };
const ack: CommandReceipt = { commandId: "cmd_original", accepted: true, firstSequence: 20, lastSequence: 20, requestFingerprint: "v1:hash" };
const cancelTurn = vi.fn<CoreClient["cancelTurn"]>(), cancelTurnReceipt = vi.fn<NonNullable<CoreClient["cancelTurnReceipt"]>>();
const client = { cancelTurn, cancelTurnReceipt } as unknown as CoreClient;
const pending = () => operations.read(task.taskId)[0];
beforeEach(() => { localStorage.clear(); vi.resetAllMocks(); cancelTurn.mockRejectedValue(new Error("offline")); cancelTurnReceipt.mockResolvedValue(null); });
afterEach(() => vi.restoreAllMocks());
it("does not send if persisting the original target fails", async () => {
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage full"); });
  await expect(operations.start(client, task)).rejects.toThrow("storage full"); expect(cancelTurn).not.toHaveBeenCalled();
});
it("recovers a lost reply without cancelling twice", async () => {
  cancelTurnReceipt.mockResolvedValueOnce(ack);
  await operations.start(client, task);
  expect(cancelTurn).toHaveBeenCalledTimes(1); expect(operations.read(task.taskId)).toEqual([]);
});
it("resends the original frozen run, turn, version and identity after reopening", async () => {
  const mutable = structuredClone(task);
  await expect(operations.start(client, mutable)).rejects.toThrow("offline"); const record = pending();
  mutable.streamVersion = 100; mutable.activePrimaryRun!.runId = "run_next";
  await expect(operations.resend(client, JSON.parse(JSON.stringify(record)))).rejects.toThrow("offline");
  expect(cancelTurn.mock.calls).toEqual([[task, record.operationId], [task, record.operationId]]);
  expect(pending()).toEqual(record);
});
it("checks acceptance without executing or depending on the current task", async () => {
  await expect(operations.start(client, task)).rejects.toThrow(); const record = pending();
  expect(await operations.check(client, record)).toBe(false); expect(cancelTurn).toHaveBeenCalledTimes(1);
  cancelTurnReceipt.mockResolvedValue(ack); expect(await operations.check(client, record)).toBe(true);
  expect(cancelTurn).toHaveBeenCalledTimes(1); expect(operations.read(task.taskId)).toEqual([]);
});
it("keeps different runs separate so recovery of the old stop cannot target the new run", async () => {
  await expect(operations.start(client, task)).rejects.toThrow(); const old = pending();
  const newer: TaskSummary = { ...task, streamVersion: 50, activePrimaryRun: { ...task.activePrimaryRun!, runId: "run_next", turnId: "trn_next" } };
  await expect(operations.start(client, newer)).rejects.toThrow();
  expect(operations.read(task.taskId)).toHaveLength(2);
  cancelTurnReceipt.mockResolvedValue(ack); await operations.check(client, old);
  expect(operations.read(task.taskId)).toHaveLength(1);
  expect(pending().task.activePrimaryRun?.runId).toBe("run_next");
  expect(cancelTurn).toHaveBeenCalledTimes(2);
});
it("does not execute if lookup fails or returns an unverified receipt", async () => {
  await expect(operations.start(client, task)).rejects.toThrow("offline");
  cancelTurn.mockClear();
  cancelTurnReceipt.mockRejectedValueOnce(new Error("connection lost"));
  await expect(operations.resend(client, pending())).rejects.toThrow("connection lost");
  cancelTurnReceipt.mockResolvedValueOnce({ ...ack, requestFingerprint: undefined });
  await expect(operations.resend(client, pending())).rejects.toThrow("无法确认");
  expect(cancelTurn).not.toHaveBeenCalled();
});

it("dispatches a fresh cancellation without a query round trip while preserving the original version", async () => {
  cancelTurn.mockResolvedValue();
  await operations.start(client, task);
  expect(cancelTurnReceipt).not.toHaveBeenCalled();
  expect(cancelTurn).toHaveBeenCalledWith(task, expect.stringMatching(/^cmd_/));
});
it("does not erase newer tracking when an old query finishes late", async () => {
  await expect(operations.start(client, task)).rejects.toThrow(); const old = pending();
  let resolve!: (value: CommandReceipt) => void;
  cancelTurnReceipt.mockReturnValueOnce(new Promise(done => { resolve = done; }));
  const check = operations.check(client, old); operations.forget(old);
  await expect(operations.start(client, { ...task, streamVersion: 8 })).rejects.toThrow();
  const newer = pending(); resolve(ack); await check;
  expect(pending()).toEqual(newer);
});
it("does not overwrite the original unresolved request on repeated clicks", async () => {
  await Promise.allSettled([operations.start(client, task), operations.start(client, task)]);
  expect(cancelTurn).toHaveBeenCalledTimes(1); expect(operations.read(task.taskId)).toHaveLength(1);
});
