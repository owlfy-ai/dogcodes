import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { projectRunClient, type ProjectRunCatalog } from "./projectRunClient";
import type { TerminalInfo } from "./terminalClient";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./availableAgentExecutors", () => ({ availableAgentExecutors: vi.fn() }));
const catalog: ProjectRunCatalog = { projectId: "project", generation: "g1", platform: "macos", shell: "sh", executor: null, directories: [] };
const info = (id: string): TerminalInfo => ({ id, epoch: "epoch", projectId: "project", directoryIndex: 0, taskId: "task", commandId: "dev", label: "dev", cwd: "/project", command: "npm run dev", status: "running", exitCode: null, error: null, outputEnd: 0, createdAt: 1 });
beforeEach(() => { vi.mocked(invoke).mockReset(); });
it("keeps the same launch identity after an uncertain reply and reattaches without relaunching", async () => {
  const exit = vi.fn(); const run = projectRunClient.launch("task", catalog, 0, "dev", exit);
  let accepted = false;
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "core_terminal_get") { if (accepted) return info(run.id); throw new Error("missing"); }
    if (command === "core_project_run_open") { accepted = true; throw new Error("reply lost"); }
    return new Promise(() => {});
  });
  await expect(run.api.open("project", 0, 80, 24, vi.fn(), "view-id")).rejects.toThrow("reply lost");
  expect(exit).not.toHaveBeenCalled();
  const connection = await run.api.open("project", 0, 100, 30, vi.fn(), "other-view-id"); connection.detach();
  const starts = vi.mocked(invoke).mock.calls.filter(([method]) => method === "core_project_run_open");
  expect(starts).toHaveLength(1); expect(starts[0][1]).toMatchObject({ operationId: run.id, cols: 80, rows: 24 });
  expect(exit).not.toHaveBeenCalled();
});
it("a stop requested before mount prevents launch", async () => {
  const exit = vi.fn(); const run = projectRunClient.launch("task", catalog, 0, "dev", exit);
  await run.stop();
  await expect(run.api.open("project", 0, 80, 24, vi.fn(), "view")).rejects.toThrow("取消");
  expect(invoke).not.toHaveBeenCalled(); expect(exit).toHaveBeenCalled();
});
it("stops an accepted process whose launch reply arrives after stop was requested", async () => {
  const exit = vi.fn(); const run = projectRunClient.launch("task", catalog, 0, "dev", exit);
  let resolve!: (value: TerminalInfo) => void;
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "core_terminal_get") throw new Error("missing");
    if (command === "core_project_run_open") return new Promise(yes => { resolve = yes as typeof resolve; });
    if (command === "core_terminal_close") return;
    return new Promise(() => {});
  });
  const opening = run.api.open("project", 0, 80, 24, vi.fn(), "view");
  for (let i = 0; i < 8; i++) await Promise.resolve();
  await run.stop(); resolve(info(run.id));
  const connection = await opening; connection.detach();
  expect(invoke).toHaveBeenCalledWith("core_terminal_close", { id: run.id });
  expect(exit).not.toHaveBeenCalled();
});
it("unknown restored processes never restart from a saved command", async () => {
  const exit = vi.fn(); const unknown = { ...info("old"), status: "unknown" as const };
  const run = projectRunClient.restore(unknown, exit);
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "core_terminal_get") return unknown;
    return { info: unknown, bytes: [], from: 0, next: 0, gap: true };
  });
  const connection = await run.api.open("project", 0, 80, 24, vi.fn(), "view"); connection.detach();
  expect(invoke).not.toHaveBeenCalledWith("core_project_run_open", expect.anything());
  expect(exit).not.toHaveBeenCalled();
});


it("never resends a previous project launch when reopening cannot find its resource", async () => {
  const run = projectRunClient.launch("task", catalog, 0, "dev", vi.fn());
  vi.mocked(invoke).mockImplementation(async command => {
    if (command === "core_terminal_get") throw new Error("resource unavailable");
    if (command === "core_project_run_open") throw new Error("reply lost");
  });
  await expect(run.api.open("project", 0, 80, 24, vi.fn(), run.id)).rejects.toThrow("reply lost");
  await expect(run.api.open("project", 0, 80, 24, vi.fn(), run.id)).rejects.toThrow("resource unavailable");
  expect(vi.mocked(invoke).mock.calls.filter(([method]) => method === "core_project_run_open")).toHaveLength(1);
});
