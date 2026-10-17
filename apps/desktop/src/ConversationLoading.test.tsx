// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ConversationLoading } from "./ConversationLoading";
import { useLoadingVisibility } from "./useLoadingVisibility";

let host: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div");
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.useRealTimers(); vi.unstubAllGlobals();
});

/** Stands in for a parent that swaps between a loader and its content. */
function Gate({ active }: { active: boolean }) {
  const loader = useLoadingVisibility(active);
  return loader.pending ? <ConversationLoading visible={loader.visible} /> : <p className="content">内容</p>;
}

it("stays blank for 300ms, so fast loads never flash a loader", async () => {
  await act(async () => root.render(<Gate active />));
  expect(host.querySelector(".conversation-loading")).not.toBeNull();
  expect(host.querySelector('[role="status"]')).toBeNull();
  await act(async () => vi.advanceTimersByTime(299));
  expect(host.querySelector('[role="status"]')).toBeNull();
  await act(async () => root.render(<Gate active={false} />));
  expect(host.querySelector(".content")).not.toBeNull();
  expect(vi.getTimerCount()).toBe(0);
});

it("once shown, holds the loader for at least 400ms before the content", async () => {
  await act(async () => root.render(<Gate active />));
  await act(async () => vi.advanceTimersByTime(300));
  expect(host.querySelector(".conversation-loading-label")?.textContent).toBe("正在加载对话…");
  await act(async () => vi.advanceTimersByTime(100));
  await act(async () => root.render(<Gate active={false} />));
  // The content is not painted for even one render while the hold lasts.
  expect(host.querySelector(".content")).toBeNull();
  expect(host.querySelector('[role="status"]')).not.toBeNull();
  await act(async () => vi.advanceTimersByTime(299));
  expect(host.querySelector(".content")).toBeNull();
  await act(async () => vi.advanceTimersByTime(1));
  expect(host.querySelector(".content")).not.toBeNull();
  expect(vi.getTimerCount()).toBe(0);
});

it("drops the hold at once when the loader has already been shown long enough", async () => {
  await act(async () => root.render(<Gate active />));
  await act(async () => vi.advanceTimersByTime(1_000));
  await act(async () => root.render(<Gate active={false} />));
  expect(host.querySelector(".content")).not.toBeNull();
});
