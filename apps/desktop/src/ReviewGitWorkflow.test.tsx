// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ReviewGitActions } from "./ReviewGitActions";
import { gitOperations, type GitReceipt, type GitRequest } from "./gitOperations";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
let host: HTMLDivElement, root: Root;
const changed = vi.fn();
beforeEach(() => { localStorage.clear(); vi.resetAllMocks(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); vi.unstubAllGlobals(); });
const click = async (text: string) => act(async () => { const button = [...host.querySelectorAll("button")].find(node => node.textContent === text); expect(button).toBeTruthy(); button!.click(); });
it("preserves the composer commit-and-push workflow for receipt-based continuation without committing twice", async () => {
  let offline = true; const writes: GitRequest[] = [];
  vi.mocked(invoke).mockImplementation(async (name, args) => {
    if (name === "core_review_git_action") { const request = args as unknown as GitRequest; writes.push(request); throw "reply lost"; }
    if (offline) throw "offline";
    if (name === "core_review_git_advance") {
      const parent = writes[0]; const request: GitRequest = { ...parent, operationId: parent.workflow!.nextOperationId, action: "push", message: "", stageAll: false, snapshotId: null, workflow: { parentOperationId: parent.operationId, nextOperationId: "retry" } };
      writes.push(request); return { request, outcome: { status: "succeeded", completed: true } };
    }
    const request = writes.find(item => item.operationId === (args as { operationId: string }).operationId);
    return request ? { request, outcome: { status: "succeeded", completed: true } } : null;
  });
  await act(async () => root.render(<ReviewGitActions taskId="task" root={{ id:"repo", path:"/repo", git:true, scopes:[""] }} snapshotId="snapshot" hasChanges disabled={false} onChanged={changed}/>));
  await act(async () => { const input = host.querySelector("input")!; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "change"); input.dispatchEvent(new Event("input", { bubbles:true })); });
  await click("提交并推送");
  expect(writes.map(item => item.action)).toEqual(["commit"]);
  const saved = gitOperations.entries("task")[0]; expect(saved.intent).toBe("commitPush");
  // Resolve the original request after the composer has been closed.
  await act(async () => root.render(null)); offline = false;
  await gitOperations.check(saved);
  await gitOperations.continuePush(saved);
  expect(writes.map(item => item.action)).toEqual(["commit", "push"]);
  expect(writes[1].operationId).toBe(saved.nextOperationId);
  expect(gitOperations.entries("task")).toEqual([]);
});
it("inspects interrupted operations without automatically retrying or discarding an unreceived request", async () => {
  vi.mocked(invoke).mockRejectedValue("offline");
  await expect(gitOperations.run({ taskId:"task", rootId:"repo", action:"pull", message:"" })).rejects.toThrow();
  const value = gitOperations.entries("task")[0];
  vi.mocked(invoke).mockReset().mockResolvedValue(null);
  expect(await gitOperations.check(value)).toBeNull();
  expect(gitOperations.entries("task")).toHaveLength(1);
  expect(vi.mocked(invoke).mock.calls.every(([name]) => name === "core_review_git_operation")).toBe(true);
  vi.mocked(invoke).mockResolvedValue({ request:value.request, outcome:{status:"unknown"} });
  expect((await gitOperations.check(value))?.outcome.status).toBe("unknown");
  expect(gitOperations.entries("task")).toHaveLength(1);
  vi.mocked(invoke).mockResolvedValue({ request:value.request, outcome:{status:"unknown"}, closed:true });
  await gitOperations.acknowledge(value);
  expect(gitOperations.entries("task")).toEqual([]);
  expect(invoke).toHaveBeenLastCalledWith("core_review_git_close", { taskId: "task", operationId: value.request.operationId });
});
it("does not label an uncertain push as failed or offer a fresh push retry", async () => {
  vi.mocked(invoke).mockImplementation(async (name, args) => {
    if (name === "core_review_git_action" && (args as unknown as GitRequest).action === "commit") return true;
    throw "offline";
  });
  await act(async () => root.render(<ReviewGitActions taskId="task" root={{ id:"repo", path:"/repo", git:true, scopes:[""] }} snapshotId="snapshot" hasChanges disabled={false} onChanged={changed}/>));
  await act(async () => { const input = host.querySelector("input")!; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "change"); input.dispatchEvent(new Event("input", { bubbles:true })); });
  await click("提交并推送");
  expect(host.textContent).toContain("推送结果待核实");
  expect(host.textContent).not.toContain("重试推送");
  const pending = gitOperations.entries("task"); expect(pending).toHaveLength(1); expect(pending[0].intent).toBe("commitPush"); expect(pending[0].request.action).toBe("commit");
});
it("keeps a rejected push recoverable after closing the composer and retries only the push", async () => {
  const writes: GitRequest[] = [];
  const receipts = new Map<string, GitReceipt>();
  vi.mocked(invoke).mockImplementation(async (name, args) => {
    if (name === "core_review_git_action") {
      const request = args as unknown as GitRequest; writes.push(request);
      receipts.set(request.operationId, { request, outcome: { status: "succeeded", completed: true } }); return true;
    }
    const id = (args as { operationId: string }).operationId;
    if (name === "core_review_git_advance") {
      const parent = receipts.get(id)!;
      const request: GitRequest = { ...parent.request, operationId: parent.request.workflow!.nextOperationId, action: "push", message: "", stageAll: false, snapshotId: null, workflow: { parentOperationId: id, nextOperationId: crypto.randomUUID() } };
      if (receipts.has(request.operationId)) return receipts.get(request.operationId);
      writes.push(request);
      const result: GitReceipt = { request, outcome: writes.length === 2 ? { status: "failed", message: "remote rejected" } : { status: "succeeded", completed: true } };
      receipts.set(request.operationId, result); return result;
    }
    return receipts.get(id) ?? null;
  });
  await act(async () => root.render(<ReviewGitActions taskId="task" root={{ id:"repo", path:"/repo", git:true, scopes:[""] }} snapshotId="snapshot" hasChanges disabled={false} onChanged={changed}/>));
  await act(async () => { const input = host.querySelector("input")!; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "change"); input.dispatchEvent(new Event("input", { bubbles:true })); });
  await click("提交并推送");
  expect([...host.querySelectorAll("button")].some(button => button.textContent === "重试推送")).toBe(false); // A failed push retains its original receipt for a durable retry.
  const saved = gitOperations.entries("task")[0];
  expect(saved.committed).toBe(true); expect(saved.receipt?.outcome.status).toBe("failed");
  await act(async () => root.render(null));
  await gitOperations.retryPush(saved);
  expect(writes.map(item => item.action)).toEqual(["commit", "push", "push"]);
  expect(writes[2].operationId).toBe(saved.retryOperationId);
  expect(gitOperations.entries("task")).toEqual([]);
});
it("retains a successful commit receipt when its continuation target is unavailable", async () => {
  vi.mocked(invoke).mockResolvedValue(true);
  await gitOperations.run({ taskId: "task", rootId: "repo", action: "commit", message: "change" }, "commitPush");
  const pending = gitOperations.entries("task")[0];
  vi.mocked(invoke).mockImplementation(async (_name, args) => (args as { operationId: string }).operationId === pending.request.operationId
    ? { request: pending.request, outcome: { status: "succeeded", completed: true, targetError: "当前分支没有上游" } } : null);
  expect((await gitOperations.check(pending))?.outcome).toEqual({ status: "succeeded", completed: true, targetError: "当前分支没有上游" });
  expect(gitOperations.entries("task")).toHaveLength(1);
});
