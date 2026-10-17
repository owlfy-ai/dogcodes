import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { watchWorkspaceDirectory, type WorkspaceWatchMessage } from "./workspaceWatch";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage = (_event: WorkspaceWatchMessage) => {}; } }));
const request = { taskId: "task", root: 0, path: "src" };
const callbacks = () => ({ refresh: vi.fn(), error: vi.fn(), scopeChanged: vi.fn() });
function watches() { return vi.mocked(invoke).mock.calls.filter(([method]) => method === "core_workspace_watch").map(([, args]) => args as { id: string; changes: { onmessage: (message: WorkspaceWatchMessage) => void } }); }
beforeEach(() => { vi.useFakeTimers(); vi.mocked(invoke).mockReset().mockResolvedValue(undefined); });
afterEach(() => { vi.useRealTimers(); });
it("reconnects with a new subscription, refreshes the snapshot, and ignores old callbacks", async () => {
  const handler = callbacks(), stop = watchWorkspaceDirectory(request, handler);
  await Promise.resolve();
  const first = watches()[0];
  expect(handler.refresh).toHaveBeenCalledTimes(1);
  first.changes.onmessage({ kind: "heartbeat" });
  expect(handler.refresh).toHaveBeenCalledTimes(1);
  first.changes.onmessage({ kind: "error", code: "disconnected", message: "offline" });
  expect(handler.error).toHaveBeenLastCalledWith(expect.stringContaining("offline"));
  expect(invoke).toHaveBeenCalledWith("core_workspace_unwatch", { id: first.id });
  await vi.advanceTimersByTimeAsync(1000);
  expect(watches()).toHaveLength(2);
  expect(watches()[1].id).not.toBe(first.id);
  expect(handler.error).toHaveBeenLastCalledWith("");
  expect(handler.refresh).toHaveBeenCalledTimes(3);
  first.changes.onmessage({ kind: "invalidated" });
  expect(handler.refresh).toHaveBeenCalledTimes(3);
  watches()[1].changes.onmessage({ kind: "invalidated" });
  expect(handler.refresh).toHaveBeenCalledTimes(4);
  stop(); await vi.advanceTimersByTimeAsync(60000);
  expect(watches()).toHaveLength(2);
});
it("cancels startup and cleans up a late acceptance without reading the directory", async () => {
  let accept!: () => void;
  vi.mocked(invoke).mockImplementation(method => method === "core_workspace_watch" ? new Promise(resolve => { accept = () => resolve(undefined); }) : Promise.resolve(undefined));
  const handler = callbacks(), stop = watchWorkspaceDirectory(request, handler);
  stop(); accept(); await Promise.resolve();
  expect(handler.refresh).not.toHaveBeenCalled();
  expect(invoke).toHaveBeenCalledWith("core_workspace_unwatch", { id: watches()[0].id });
  await vi.advanceTimersByTimeAsync(60000); expect(watches()).toHaveLength(1);
});
it("retries failed setup with backoff and stops retries on disposal", async () => {
  vi.mocked(invoke).mockImplementation(method => method === "core_workspace_watch" ? Promise.reject("unavailable") : Promise.resolve(undefined));
  const handler = callbacks(), stop = watchWorkspaceDirectory(request, handler);
  await vi.advanceTimersByTimeAsync(0);
  expect(handler.refresh).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1000); expect(watches()).toHaveLength(2);
  await vi.advanceTimersByTimeAsync(1999); expect(watches()).toHaveLength(2);
  await vi.advanceTimersByTimeAsync(1); expect(watches()).toHaveLength(3);
  stop(); await vi.advanceTimersByTimeAsync(60000); expect(watches()).toHaveLength(3);
});
it("refreshes root scope on change and never treats a heartbeat as a file change", async () => {
  const handler = callbacks(), stop = watchWorkspaceDirectory(request, handler);
  await Promise.resolve();
  watches()[0].changes.onmessage({ kind: "error", code: "scope_changed", message: "directory changed" });
  expect(handler.scopeChanged).toHaveBeenCalledOnce();
  stop();
});
