// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { useConversationReview } from "./useConversationReview";

it("keeps one sidebar toggle across cached and unvisited conversations without flashes", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let state!: ReturnType<typeof useConversationReview>;
  const frames: boolean[] = [];
  function Test({ taskId }: { taskId: string | null }) {
    state = useConversationReview(taskId);
    frames.push(state[0]);
    return state[0] ? <aside>Review</aside> : null;
  }
  const element = document.createElement("div");
  const root = createRoot(element);
  const select = async (taskId: string | null) => {
    frames.length = 0;
    await act(async () => root.render(<Test taskId={taskId} />));
  };
  try {
    await select("a");
    expect(state[0]).toBe(false);
    await act(async () => state[1](true));
    expect(element.querySelector("aside")).not.toBeNull();

    await select("b");
    expect(frames.every(Boolean)).toBe(true);
    expect(element.querySelector("aside")).not.toBeNull();
    await select("a");
    expect(frames.every(Boolean)).toBe(true);
    await act(async () => state[1](false));
    await select("b");
    expect(frames.every(open => !open)).toBe(true);
    await select("unvisited");
    expect(frames.every(open => !open)).toBe(true);
    await select("a");
    expect(state[0]).toBe(false);

    await select(null);
    await act(async () => state[1](true));
    expect(state[0]).toBe(false);
    await select("b");
    expect(state[0]).toBe(false);
    await act(async () => state[1](open => !open));
    expect(state[0]).toBe(true);
    await select(null);
    expect(element.querySelector("aside")).toBeNull();
    await select("unvisited");
    expect(frames.every(Boolean)).toBe(true);
    await select("never-opened");
    expect(frames.every(Boolean)).toBe(true);
  } finally {
    await act(async () => root.unmount());
    vi.unstubAllGlobals();
  }
});
