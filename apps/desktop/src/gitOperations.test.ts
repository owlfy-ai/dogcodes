// @vitest-environment jsdom
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { gitOperations, type GitRequest, type GitReceipt } from "./gitOperations";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const input = { taskId: "task", rootId: "repo", action: "commit" as const, message: "change", snapshotId: "snapshot" };
beforeEach(() => { localStorage.clear(); vi.resetAllMocks(); });
afterEach(() => { vi.restoreAllMocks(); localStorage.clear(); });
it("persists before sending and resolves a lost reply from its receipt without resending", async () => {
  let request!: GitRequest;
  vi.mocked(invoke).mockImplementation(async (method, args) => {
    if (method === "core_review_git_action") { request = args as unknown as GitRequest; expect(gitOperations.entries("task")[0].request).toEqual(request); throw "reply lost"; }
    return { request, outcome: { status: "succeeded", completed: true } };
  });
  await expect(gitOperations.run(input)).resolves.toBe(true);
  expect(vi.mocked(invoke).mock.calls.filter(([method]) => method === "core_review_git_action")).toHaveLength(1);
  expect(gitOperations.entries("task")).toEqual([]);
});
it("keeps an uncertain original payload and only explicitly resends the same identity when absent", async () => {
  vi.mocked(invoke).mockRejectedValue("offline");
  await expect(gitOperations.run(input)).rejects.toThrow("尚未核实");
  const pending = gitOperations.entries("task")[0];
  await expect(gitOperations.run({ ...input, message: "different" })).rejects.toThrow("尚未核实");
  vi.mocked(invoke).mockImplementation(async method => method === "core_review_git_operation" ? null : true);
  await expect(gitOperations.resend(pending)).resolves.toBe(true);
  const calls = vi.mocked(invoke).mock.calls.filter(([name]) => name === "core_review_git_action");
  expect(calls).toHaveLength(2); expect(calls[1][1]).toEqual(calls[0][1]);
});
it.each(["running", "unknown"] as const)("does not replay a %s intent", async status => {
  vi.mocked(invoke).mockRejectedValue("offline"); await expect(gitOperations.run(input)).rejects.toThrow();
  const pending = gitOperations.entries("task")[0];
  vi.mocked(invoke).mockReset().mockResolvedValue({ request: pending.request, outcome: { status } });
  await expect(gitOperations.resend(pending)).rejects.toThrow("尚未核实");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_operation", { taskId: "task", operationId: pending.request.operationId });
  expect(gitOperations.entries("task")).toHaveLength(1);
});
it("persists combination intent and advances from a different client without local state", async () => {
  let commit!: GitReceipt;
  let child!: GitReceipt;
  vi.mocked(invoke).mockImplementation(async (name, args) => {
    if (name === "core_review_git_action") {
      const request = args as unknown as GitRequest;
      expect(request.workflow?.nextOperationId).toBeTruthy();
      commit = { request, outcome: { status: "succeeded", completed: true } }; return true;
    }
    if (name === "core_review_git_operation") return commit;
    if (name === "core_review_git_advance") {
      child ??= { request: { ...commit.request, operationId: commit.request.workflow!.nextOperationId, action: "push", message: "", stageAll: false, snapshotId: null,
        workflow: { parentOperationId: commit.request.operationId, nextOperationId: "retry" } }, outcome: { status: "succeeded", completed: true } };
      return child;
    }
    throw new Error(`unexpected ${name}`);
  });
  await gitOperations.run(input, "commitPush");
  const saved = gitOperations.entries("task")[0];
  localStorage.clear(); // Another client has only the backend receipt.
  expect(await gitOperations.advanceFromReceipt(commit)).toEqual(child);
  await gitOperations.advance("task", saved.request.operationId); // Late original caller.
  const advances = vi.mocked(invoke).mock.calls.filter(([name]) => name === "core_review_git_advance");
  expect(advances).toHaveLength(2); expect(advances[0][1]).toEqual(advances[1][1]);
  expect(vi.mocked(invoke).mock.calls.filter(([name]) => name === "core_review_git_action")).toHaveLength(1);
  expect(gitOperations.entries("task")).toEqual([]);
});
it("distinguishes stage confirmation and known failure from unknown results", async () => {
  vi.mocked(invoke).mockResolvedValue(false); expect(await gitOperations.run(input, "commitPush")).toBe(false); expect(gitOperations.entries("task")).toEqual([]);
  let request!: GitRequest;
  vi.mocked(invoke).mockImplementation(async (name, args) => {
    if (name === "core_review_git_action") { request = args as unknown as GitRequest; throw "transport"; }
    return { request, outcome: { status: "failed", message: "pre-commit failed" } } satisfies GitReceipt;
  });
  await expect(gitOperations.run(input)).rejects.toThrow("pre-commit failed"); expect(gitOperations.entries("task")).toEqual([]);
});
it("will not run without durable client identity or accept a mismatched receipt", async () => {
  const save = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage unavailable"); });
  await expect(gitOperations.run(input)).rejects.toThrow("storage unavailable"); expect(invoke).not.toHaveBeenCalled(); save.mockRestore();
  vi.mocked(invoke).mockRejectedValue("offline"); await expect(gitOperations.run(input)).rejects.toThrow();
  const pending = gitOperations.entries("task")[0];
  vi.mocked(invoke).mockReset().mockResolvedValue({ request: { ...pending.request, message: "different" }, outcome: { status: "succeeded", completed: true } });
  await expect(gitOperations.resend(pending)).rejects.toThrow("不匹配"); expect(invoke).toHaveBeenCalledOnce(); expect(gitOperations.entries("task")).toHaveLength(1);
});
it("retains a failed push after a commit and shares its reserved retry ID across recovery views", async () => {
  const push = { taskId: "task", rootId: "repo", action: "push" as const, message: "" };
  const receipts = new Map<string, GitReceipt>();
  const writes: GitRequest[] = [];
  vi.mocked(invoke).mockImplementation(async (name, args) => {
    if (name === "core_review_git_operation") return receipts.get((args as { operationId: string }).operationId) ?? null;
    const request = args as unknown as GitRequest; writes.push(request);
    if (writes.length === 1) {
      receipts.set(request.operationId, { request, outcome: { status: "failed", message: "remote rejected" } });
      throw "remote rejected";
    }
    receipts.set(request.operationId, { request, outcome: { status: "succeeded", completed: true } });
    return true;
  });
  await expect(gitOperations.run(push, "push", { committed: true })).rejects.toThrow("remote rejected");
  const saved = gitOperations.entries("task")[0];
  expect(saved.receipt?.outcome.status).toBe("failed"); expect(saved.retryOperationId).toBeTruthy();
  await expect(gitOperations.run(push)).rejects.toThrow("尚未核实");
  await expect(gitOperations.retryPush(saved)).resolves.toBe(true);
  // A second recovery view may still hold the old failed receipt. Its explicit
  // retry must reach the backend with the same ID, which the journal deduplicates.
  await expect(gitOperations.retryPush(saved)).resolves.toBe(true);
  expect(writes.map(item => item.action)).toEqual(["push", "push", "push"]);
  expect(writes[1].operationId).toBe(saved.retryOperationId);
  expect(writes[2]).toEqual(writes[1]);
  expect(gitOperations.entries("task")).toEqual([]);
});
it("does not turn an unknown push into a new retry operation", async () => {
  vi.mocked(invoke).mockRejectedValue("offline");
  await expect(gitOperations.run({ taskId: "task", rootId: "repo", action: "push", message: "" }, "push", { committed: true })).rejects.toThrow();
  const pending = gitOperations.entries("task")[0];
  vi.mocked(invoke).mockReset().mockResolvedValue({ request: pending.request, outcome: { status: "unknown" } });
  await expect(gitOperations.retryPush(pending)).rejects.toThrow("尚未核实");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_operation", { taskId: "task", operationId: pending.request.operationId });
});
it("keeps tracking when closure is unconfirmed and resolves a lost close reply from the backend", async () => {
  vi.mocked(invoke).mockRejectedValue("offline");
  await expect(gitOperations.run(input)).rejects.toThrow();
  const pending = gitOperations.entries("task")[0];
  vi.mocked(invoke).mockImplementation(async name => {
    if (name === "core_review_git_close") throw "reply lost";
    return { request: pending.request, outcome: { status: "unknown" }, closed: false };
  });
  await expect(gitOperations.acknowledge(pending)).rejects.toThrow("尚未确认");
  expect(gitOperations.entries("task")).toHaveLength(1);
  vi.mocked(invoke).mockImplementation(async name => {
    if (name === "core_review_git_close") throw "reply lost";
    return { request: pending.request, outcome: { status: "unknown" }, closed: true };
  });
  await gitOperations.acknowledge(pending);
  expect(gitOperations.entries("task")).toEqual([]);
});
it("honors closure by another client without issuing a continuation", async () => {
  vi.mocked(invoke).mockResolvedValue(true);
  await gitOperations.run(input, "commitPush");
  const pending = gitOperations.entries("task")[0];
  vi.mocked(invoke).mockReset().mockResolvedValue({ request: pending.request, outcome: { status: "succeeded", completed: true }, closed: true });
  await expect(gitOperations.continuePush(pending)).rejects.toThrow("已在后台结束");
  expect(vi.mocked(invoke).mock.calls.map(([name]) => name)).toEqual(["core_review_git_operation"]);
  expect(gitOperations.entries("task")).toEqual([]);
});
