// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, ForkRequest } from "@dogcodes/protocol";
import { forkOperations as operations } from "./forkOperations";
const request: ForkRequest = { taskId: "tsk_source", turnId: "trn_source", newTaskId: "tsk_target", environment: "worktree" };
const forkTask = vi.fn(), forkReceipt = vi.fn();
const client = { forkTask, forkReceipt } as unknown as CoreClient;
beforeEach(() => { localStorage.clear(); vi.resetAllMocks(); forkTask.mockRejectedValue(new Error("lost")); forkReceipt.mockResolvedValue(null); });
afterEach(() => vi.restoreAllMocks());
it("does not invoke Fork when persisting the original request fails", async () => {
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage full"); });
  await expect(operations.send(client, request)).rejects.toThrow("storage full");
  expect(forkTask).not.toHaveBeenCalled();
});
it("preserves the original environment and target when the outcome is unknown", async () => {
  await expect(operations.send(client, request)).rejects.toThrow("原 Fork 请求已保留");
  await expect(operations.send(client, { ...request, environment: "workspace" })).rejects.toThrow("不能更换");
  await expect(operations.send(client, { ...request, newTaskId: "tsk_other" })).rejects.toThrow("不能更换");
  expect(forkTask).toHaveBeenCalledTimes(1); expect(operations.read(request.taskId, request.turnId)).toEqual(request);
});
it("does not resend if the receipt belongs to a different source turn", async () => {
  await expect(operations.send(client, request)).rejects.toThrow();
  forkReceipt.mockResolvedValue({ request: { ...request, turnId: "trn_other" }, taskAvailable: true, completed: true });
  await expect(operations.resend(client, request)).rejects.toThrow("不一致");
  expect(forkTask).toHaveBeenCalledTimes(1);
});
it("does not recreate a previously created but unavailable task", async () => {
  forkReceipt.mockResolvedValue({ request, taskAvailable: false, completed: true });
  await expect(operations.resend(client, request)).rejects.toThrow("目标会话已不可用");
  expect(forkTask).not.toHaveBeenCalled();
});
it("keeps an accepted request until its result has actually been opened", async () => {
  forkTask.mockResolvedValue(request.newTaskId);
  expect(await operations.send(client, request)).toBe(request.newTaskId);
  expect(operations.read(request.taskId, request.turnId)).toEqual(request);
  operations.forget(request); expect(operations.read(request.taskId, request.turnId)).toBeNull();
});
it("retries an incomplete operation with its original identity without treating it as created", async () => {
  forkReceipt.mockResolvedValue({ request, completed: false, taskAvailable: false });
  forkTask.mockResolvedValue(request.newTaskId);
  expect(await operations.resend(client, request)).toBe(request.newTaskId);
  expect(forkTask).toHaveBeenCalledExactlyOnceWith(request);
});
it("does not retry an operation that is still executing on the host", async () => {
  forkReceipt.mockResolvedValue({ request, completed: false, taskAvailable: false, status: "running" });
  await expect(operations.resend(client, request)).rejects.toThrow("仍在后台执行");
  expect(forkTask).not.toHaveBeenCalled();
});
it("retires only the matching local intent after a canonical source deletion", async () => {
  await expect(operations.send(client, request)).rejects.toThrow();
  const taskDeletion = vi.fn(async () => ({ taskId: request.taskId, deletionSequence: 12 }));
  await expect(operations.resend({ ...client, taskDeletion }, request)).rejects.toThrow("源会话已永久删除");
  expect(operations.read(request.taskId, request.turnId)).toBeNull();
  expect(forkTask).toHaveBeenCalledTimes(1);
});
it("keeps the original intent when deletion cannot be verified", async () => {
  await expect(operations.send(client, request)).rejects.toThrow();
  const taskDeletion = vi.fn(async () => { throw new Error("offline"); });
  await expect(operations.resend({ ...client, taskDeletion }, request)).rejects.toThrow("offline");
  expect(operations.read(request.taskId, request.turnId)).toEqual(request);
  expect(forkTask).toHaveBeenCalledTimes(1);
});
