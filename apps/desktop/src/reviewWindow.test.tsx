// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useReviewWindow } from "./reviewWindow";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => true }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
it("docks only the active task and reports failed native window creation", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let closed!: (event: { payload: string }) => void;
  const stop = vi.fn();
  vi.mocked(listen).mockImplementation(async (_name, handler) => { closed = handler as typeof closed; return stop; });
  vi.mocked(invoke).mockResolvedValue(false);
  const onDock = vi.fn();
  let state!: ReturnType<typeof useReviewWindow>;
  function Test({ task }: { task: string }) { state = useReviewWindow(task, onDock); return null; }
  const element = document.createElement("div"); const root = createRoot(element);
  try {
    await act(async () => root.render(<Test task="one" />));
    await act(async () => state.detach());
    expect(state.detached).toBe(true);
    await act(async () => root.render(<Test task="two" />));
    await act(async () => closed({ payload: "one" }));
    expect(onDock).not.toHaveBeenCalled();
    await act(async () => closed({ payload: "two" }));
    expect(onDock).toHaveBeenCalledTimes(1);
    vi.mocked(invoke).mockRejectedValueOnce(new Error("window unavailable"));
    await act(async () => state.detach());
    expect(state.detached).toBe(false);
    expect(state.error).toContain("无法打开独立窗口");
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
  expect(stop).toHaveBeenCalled();
});
