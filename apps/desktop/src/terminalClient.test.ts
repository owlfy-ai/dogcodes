import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { followTerminal, terminalClient, type TerminalInfo, type OutputBatch } from "./terminalClient";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const info: TerminalInfo = { id: "one", epoch: "epoch", projectId: "project", directoryIndex: 0, taskId: null, commandId: null, label: "shell", cwd: "/project", command: null, status: "running", exitCode: null, error: null, outputEnd: 0, createdAt: 1 };
beforeEach(() => { vi.mocked(invoke).mockReset(); });
const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
it("drains buffered output after exit and preserves the byte cursor and gap", async () => {
  const ended = { ...info, status: "exited" as const, outputEnd: 4, exitCode: 7 };
  vi.mocked(invoke).mockResolvedValueOnce({ info: ended, bytes: [65], from: 2, next: 3, gap: true } satisfies OutputBatch)
    .mockResolvedValueOnce({ info: ended, bytes: [66], from: 3, next: 4, gap: false } satisfies OutputBatch);
  const receive = vi.fn(); followTerminal(info, receive); await flush();
  expect(invoke).toHaveBeenNthCalledWith(1, "core_terminal_read", { id: "one", after: 0 });
  expect(invoke).toHaveBeenNthCalledWith(2, "core_terminal_read", { id: "one", after: 3 });
  expect(invoke).toHaveBeenCalledTimes(2);
  expect(receive.mock.calls.map(([event]) => event.kind)).toEqual(["state", "gap", "data", "state", "data"]);
});
it("detach discards the outstanding read and never sends stop", async () => {
  let resolve!: (value: OutputBatch) => void;
  vi.mocked(invoke).mockImplementation(() => new Promise(yes => { resolve = yes as typeof resolve; }));
  const receive = vi.fn(); const connection = followTerminal(info, receive);
  connection.detach(); receive.mockClear();
  resolve({ info, bytes: [65], from: 0, next: 1, gap: false }); await flush();
  expect(receive).not.toHaveBeenCalled(); expect(invoke).toHaveBeenCalledTimes(1);
});
it("reports transport failure without a fabricated exit or automatic restart", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("offline"));
  const receive = vi.fn(); followTerminal(info, receive); await flush();
  expect(receive.mock.calls.map(([event]) => event.kind)).toEqual(["state", "disconnected"]);
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("reattaches an already accepted launch even if the original response was lost", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(info).mockImplementation(() => new Promise(() => {}));
  const connection = await terminalClient.open("project", 0, 80, 24, vi.fn(), "one");
  connection.detach();
  expect(invoke).not.toHaveBeenCalledWith("core_terminal_open", expect.anything());
  expect(connection.id).toBe("one");
});

it("closes only the observed epoch and never queries a replacement target", async () => {
  vi.mocked(invoke).mockResolvedValue(undefined);
  await terminalClient.close(info.id, info.epoch);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_terminal_close", { id: info.id, expectedEpoch: info.epoch });
});
it.each([{ id: "other" }, { epoch: "next-core" }])("rejects output from another terminal target before displaying bytes: %j", async changed => {
  vi.mocked(invoke).mockResolvedValue({ info: { ...info, ...changed }, bytes: [65], from: 0, next: 1, gap: true });
  const receive = vi.fn(); const connection = followTerminal(info, receive);
  await flush(); connection.detach();
  expect(receive.mock.calls.map(([event]) => event.kind)).toEqual(["state", "disconnected"]);
  expect(invoke).toHaveBeenCalledTimes(1);
});

it("freezes the stream target before sharing the initial info with the view", async () => {
  const shared = { ...info };
  vi.mocked(invoke).mockImplementation(async () => ({ info: shared, bytes: [65], from: 0, next: 1, gap: false }));
  const kinds: string[] = [];
  const connection = followTerminal(shared, event => {
    kinds.push(event.kind);
    if (event.kind === "state") { shared.epoch = "replacement"; shared.id = "replacement"; }
  });
  await flush(); connection.detach();
  expect(kinds).toEqual(["state", "disconnected"]);
  expect(connection.id).toBe(info.id);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_terminal_read", { id: info.id, after: 0 });
});
