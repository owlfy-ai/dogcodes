// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { reviewClient } from "./reviewClient";
import { reviewMutations, type MutationRequest } from "./reviewMutations";
import { ReviewMutationRecovery } from "./ReviewMutationRecovery";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./reviewWatch", () => ({ subscribeReviewChanges: () => () => {} }));
let host: HTMLDivElement, root: Root;
beforeEach(() => { localStorage.clear(); vi.resetAllMocks(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const discard = { taskId: "task", snapshotId: "snapshot", rootId: "repo", path: "a", confirmed: true };
const batch = { taskId: "task", snapshotId: "snapshot", rootId: "repo", paths: ["a", "b"], action: "discard" as const, confirmed: true };
const click = async (label: string) => act(async () => { const button = [...host.querySelectorAll("button")].find(button => button.textContent === label); expect(button).toBeTruthy(); button!.click(); });

it("binds each real client mutation to its persisted identity and recovers lost replies", async () => {
  let original!: MutationRequest;
  const writes: string[] = [];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "core_review_file_operation") return { request: original, outcome: { status: "succeeded", result: original.command === "core_review_discard" ? "backup-id" : null } };
    original = reviewMutations.entries("task")[0].request;
    expect(args).toEqual({ ...original.arguments, operationId: original.operationId });
    writes.push(command); throw "reply lost";
  });
  await reviewClient.index("task", "snapshot", "repo", "a", "stage");
  await reviewClient.index("task", "snapshot", "repo", ["a", "b"], "unstage");
  await reviewClient.ignore("task", "snapshot", "repo", "a");
  expect(await reviewClient.discard(discard)).toBe("backup-id");
  expect(writes).toEqual(["core_review_index", "core_review_index", "core_review_ignore", "core_review_discard"]);
  expect(reviewMutations.entries("task")).toEqual([]);
});
it("preserves a partial batch across reopen and never replays already completed members", async () => {
  vi.mocked(invoke).mockRejectedValue("offline");
  await expect(reviewClient.batchFiles(batch)).rejects.toThrow("尚未核实");
  const saved = reviewMutations.entries("task")[0];
  vi.mocked(invoke).mockReset().mockResolvedValue({ request: saved.request, outcome: { status: "succeeded", result: { completed: ["a"], error: "b changed" } } });
  const changed = vi.fn();
  await act(async () => root.render(<ReviewMutationRecovery taskId="task" onChanged={changed}/>));
  expect(host.textContent).toContain("已完成 1 个文件，部分未完成：b changed");
  expect(host.querySelector("li")?.textContent).toBe("a");
  expect(host.textContent).not.toContain("重发原请求");
  await expect(reviewClient.batchFiles(batch)).rejects.toThrow("尚未核实");
  await act(async () => root.render(null));
  await act(async () => root.render(<ReviewMutationRecovery taskId="task" onChanged={changed}/>));
  vi.mocked(invoke).mockResolvedValue({ request: saved.request, outcome: { status: "succeeded", result: { completed: ["a"], error: "b changed" } }, closed: true });
  await click("已核对文件，结束此次操作");
  expect(reviewMutations.entries("task")).toEqual([]);
  expect(vi.mocked(invoke).mock.calls.every(([command]) => command === "core_review_file_operation" || command === "core_review_file_close")).toBe(true);
  expect(changed).toHaveBeenCalledOnce();
});
it("allows only an explicit exact resend when the service has no receipt yet", async () => {
  const writes: unknown[] = [];
  let online = false;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "core_review_file_operation") return null;
    writes.push(args); if (!online) throw "offline"; return "backup";
  });
  await expect(reviewClient.discard(discard)).rejects.toThrow("尚未核实");
  await act(async () => root.render(<ReviewMutationRecovery taskId="task" onChanged={() => {}}/>));
  expect(host.textContent).not.toContain("结束此次操作");
  expect(writes).toHaveLength(1);
  online = true; await click("重发原请求");
  expect(writes).toHaveLength(2); expect(writes[1]).toEqual(writes[0]);
  expect(reviewMutations.entries("task")).toEqual([]);
});
it.each(["running", "unknown"])("does not reexecute a %s mutation", async status => {
  vi.mocked(invoke).mockRejectedValue("offline"); await expect(reviewClient.discard(discard)).rejects.toThrow();
  const saved = reviewMutations.entries("task")[0];
  vi.mocked(invoke).mockReset().mockResolvedValue({ request: saved.request, outcome: { status } });
  await expect(reviewMutations.resend(saved)).rejects.toThrow("尚未核实");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_file_operation", { taskId: "task", operationId: saved.request.operationId });
});
it("rejects mismatched receipts and does not execute when local persistence fails", async () => {
  const save = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage full"); });
  await expect(reviewClient.discard(discard)).rejects.toThrow("storage full"); expect(invoke).not.toHaveBeenCalled(); save.mockRestore();
  vi.mocked(invoke).mockRejectedValue("offline"); await expect(reviewClient.discard(discard)).rejects.toThrow();
  const saved = reviewMutations.entries("task")[0];
  vi.mocked(invoke).mockReset().mockResolvedValue({ request: { ...saved.request, arguments: { request: { ...discard, path: "other" } } }, outcome: { status: "succeeded", result: "backup" } });
  await expect(reviewMutations.resend(saved)).rejects.toThrow("不匹配"); expect(invoke).toHaveBeenCalledOnce();
});
it("retains known failures and partial live results for explicit inspection", async () => {
  vi.mocked(invoke).mockResolvedValue({ completed: ["a"], error: "b changed" });
  expect(await reviewClient.batchFiles(batch)).toEqual({ completed: ["a"], error: "b changed" });
  const partial = reviewMutations.entries("task")[0]; expect(partial.receipt?.outcome.status).toBe("succeeded");
  vi.mocked(invoke).mockResolvedValue({ ...partial.receipt, closed: true });
  await reviewMutations.acknowledge(partial);
  vi.mocked(invoke).mockImplementation(async command => {
    if (command !== "core_review_file_operation") throw "read-only";
    return { request: reviewMutations.entries("task")[0].request, outcome: { status: "failed", message: "read-only filesystem" } };
  });
  await expect(reviewClient.discard(discard)).rejects.toThrow("read-only filesystem");
  expect(reviewMutations.entries("task")[0].receipt?.outcome.status).toBe("failed");
});
it("keeps tracking until a shared closure is proven, including a lost close reply", async () => {
  vi.mocked(invoke).mockResolvedValue({ completed: ["a"], error: "b changed" });
  await reviewClient.batchFiles(batch);
  const pending = reviewMutations.entries("task")[0];
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "core_review_file_close") throw new Error("reply lost");
    return { ...pending.receipt, closed: true };
  });
  await reviewMutations.acknowledge(pending);
  expect(reviewMutations.entries("task")).toEqual([]);
  expect(invoke).toHaveBeenCalledWith("core_review_file_close", { taskId: "task", operationId: pending.request.operationId });
});
it("removes another client's closed tracking only after reading a matching receipt", async () => {
  vi.mocked(invoke).mockResolvedValue({ completed: ["a"], error: "b changed" });
  await reviewClient.batchFiles(batch);
  const pending = reviewMutations.entries("task")[0];
  vi.mocked(invoke).mockRejectedValue("offline");
  await expect(reviewMutations.acknowledge(pending)).rejects.toThrow();
  expect(reviewMutations.entries("task")).toHaveLength(1);
  vi.mocked(invoke).mockResolvedValue({ ...pending.receipt, closed: true });
  await reviewMutations.check(pending);
  expect(reviewMutations.entries("task")).toEqual([]);
});
