// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { runOperations } from "./projectRunOperations";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const catalog = { projectId: "project", generation: "new", directories: [] };
const request = { taskId: "task", operationId: "original", generation: "old", directoryIndex: 0, commandId: "dev", edit: { action: "pin", pinned: true } };
beforeEach(() => { localStorage.clear(); vi.mocked(invoke).mockReset(); });
it("recovers a lost mutation reply from the receipt without resending", async () => {
  vi.mocked(invoke).mockRejectedValueOnce(new Error("lost reply")).mockResolvedValueOnce({ status: "succeeded" }).mockResolvedValueOnce(catalog);
  expect(await runOperations.mutate("core_project_run_edit", request)).toEqual(catalog);
  expect(vi.mocked(invoke).mock.calls.map(([method]) => method)).toEqual(["core_project_run_edit", "core_project_run_operation", "core_project_runs"]);
  expect(runOperations.hasPending("task")).toBe(false);
});
it("settles a request left by a closed window from the service record, never resending it", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("offline"));
  await expect(runOperations.mutate("core_project_run_edit", request)).rejects.toThrow("offline");
  expect(runOperations.hasPending("task")).toBe(true);
  vi.resetModules();
  const reopened = (await import("./projectRunOperations")).runOperations;
  expect(reopened.hasPending("task")).toBe(true);
  vi.mocked(invoke).mockReset().mockResolvedValueOnce(null);
  await reopened.settlePending("task");
  expect(vi.mocked(invoke).mock.calls).toEqual([["core_project_run_operation", { taskId: "task", operationId: "original" }]]);
  expect(reopened.hasPending("task")).toBe(false);
});
it("settles the earlier request before sending a new one, and refuses while it can't be read", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("offline"));
  await expect(runOperations.mutate("core_project_run_edit", request)).rejects.toThrow("offline");
  await expect(runOperations.mutate("core_project_run_edit", { ...request, operationId: "second" })).rejects.toThrow("还在确认中");
  expect(vi.mocked(invoke).mock.calls.filter(([method]) => method === "core_project_run_edit")).toHaveLength(1);
  vi.mocked(invoke).mockReset().mockResolvedValueOnce({ status: "succeeded" }).mockResolvedValueOnce(catalog);
  expect(await runOperations.mutate("core_project_run_edit", { ...request, operationId: "second" })).toEqual(catalog);
  expect(runOperations.hasPending("task")).toBe(false);
});
it("an older reply cannot clear another window's pending request", async () => {
  let finish!: (value: unknown) => void;
  vi.mocked(invoke).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const first = runOperations.mutate("core_project_run_edit", request);
  localStorage.setItem("dogcodes.project-run.pending.task.second", JSON.stringify({ method: "core_project_run_edit", request: { ...request, operationId: "second" } }));
  finish(catalog); await first;
  expect(runOperations.hasPending("task")).toBe(true);
  expect(localStorage.getItem("dogcodes.project-run.pending.task.second")).not.toBeNull();
});
it("keeps the request when its record can't be read yet", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("offline"));
  await expect(runOperations.mutate("core_project_run_edit", request)).rejects.toThrow();
  vi.mocked(invoke).mockClear();
  await expect(runOperations.settlePending("task")).rejects.toThrow();
  expect(invoke).toHaveBeenCalledTimes(1); expect(runOperations.hasPending("task")).toBe(true);
});
it("drops an unreadable record instead of blocking run commands", () => {
  localStorage.setItem("dogcodes.project-run.pending.task.bad", "{");
  expect(runOperations.hasPending("task")).toBe(true);
  return runOperations.settlePending("task").then(() => expect(runOperations.hasPending("task")).toBe(false));
});
it("clears only its request on an explicit rejection without querying or resending", async () => {
  vi.mocked(invoke).mockImplementationOnce(async () => {
    localStorage.setItem("dogcodes.project-run.pending.task.second", "another window");
    throw { kind: "rejected", code: "version_conflict", message: "运行项已更新，请刷新后重试" };
  });
  await expect(runOperations.mutate("core_project_run_edit", request)).rejects.toMatchObject({ code: "version_conflict", message: "运行项已更新，请刷新后重试" });
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(localStorage.getItem("dogcodes.project-run.pending.task.original")).toBeNull();
  expect(localStorage.getItem("dogcodes.project-run.pending.task.second")).toBe("another window");
});
it("keeps unknown storage outcomes pending and displays their message", async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ kind: "unknown", message: "运行配置写入结果待核实" }).mockResolvedValueOnce(null);
  await expect(runOperations.mutate("core_project_run_edit", request)).rejects.toThrow("运行配置写入结果待核实");
  expect(runOperations.hasPending("task")).toBe(true);
  expect(invoke).toHaveBeenCalledTimes(2);
});
it("does not infer rejection from text that resembles a backend code", async () => {
  vi.mocked(invoke).mockRejectedValueOnce("run_commands_confirmation_required").mockResolvedValueOnce(null);
  await expect(runOperations.mutate("core_project_runs_analyze", request)).rejects.toBe("run_commands_confirmation_required");
  expect(runOperations.hasPending("task")).toBe(true);
});
