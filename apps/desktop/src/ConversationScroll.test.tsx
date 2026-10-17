// @vitest-environment jsdom
import { act, useLayoutEffect } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { ConversationScroll } from "./ConversationScroll";

it("positions history before paint, follows resizes, and respects scrolling up", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let resize!: () => void;
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { resize = callback; }
    observe() {} disconnect() {}
  });
  let height = 1400;
  const scrollHeight = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const clientHeight = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const host = document.createElement("div"), root = createRoot(host);
  const beforePaint: number[] = [];
  function Screen({ taskId }: { taskId: string }) {
    useLayoutEffect(() => { beforePaint.push(host.querySelector(".timeline")!.scrollTop); }, [taskId]);
    return <ConversationScroll taskId={taskId}><p>Long history</p></ConversationScroll>;
  }
  try {
    await act(async () => root.render(<Screen taskId="one" />));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    expect(beforePaint).toEqual([1400]);
    height = 1600;
    await act(async () => resize());
    expect(node.scrollTop).toBe(1600);
    await act(async () => {
      node.dispatchEvent(new WheelEvent("wheel", { deltaY: -400, bubbles: true }));
      node.scrollTop = 600;
      node.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    expect(host.textContent).toContain("回到底部");
    height = 1800;
    await act(async () => resize());
    expect(node.scrollTop).toBe(600);
    await act(async () => root.render(<Screen taskId="two" />));
    expect(beforePaint).toEqual([1400, 1800]);
    expect(host.textContent).not.toContain("回到底部");
  } finally {
    await act(async () => root.unmount());
    scrollHeight.mockRestore(); clientHeight.mockRestore(); vi.unstubAllGlobals();
  }
});

it("reveals the latest summary despite trailing actions and restores its previous position on collapse", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let resize!: () => void;
  vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect() {} });
  let height = 1400;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const host = document.createElement("div"), root = createRoot(host);
  const card = (name: string) => <div><details className="turn-summary"><summary className="turn-summary-heading"><strong>{name}</strong><button onClick={event => event.preventDefault()}>Undo</button></summary><p>Files</p></details></div>;
  try {
    await act(async () => root.render(<ConversationScroll taskId="one">{card("old")}{card("latest")}<div>Turn actions</div></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    const headings = host.querySelectorAll<HTMLElement>("summary");
    node.scrollTop = 600;
    await act(async () => {
      headings[1].querySelector("button")!.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      headings[1].querySelector("button")!.click();
    });
    expect(headings[1].parentElement!.hasAttribute("open")).toBe(false);
    expect(node.scrollTop).toBe(600);
    await act(async () => {
      headings[1].dispatchEvent(new Event("pointerdown", { bubbles: true }));
      const toggled = new Promise<void>(resolve => headings[1].parentElement!.addEventListener("toggle", () => resolve(), { once: true }));
      headings[1].querySelector("strong")!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
      height = 1700; resize();
      await toggled;
    });
    expect(node.scrollTop).toBe(1700);
    node.scrollTop = 1200;
    await act(async () => {
      headings[1].dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));
      const toggled = new Promise<void>(resolve => headings[1].parentElement!.addEventListener("toggle", () => resolve(), { once: true }));
      headings[1].click(); height = 1400; resize();
      await toggled;
    });
    expect(node.scrollTop).toBe(600);
    await act(async () => { node.dispatchEvent(new Event("scroll")); resize(); });
    expect(node.scrollTop).toBe(600);
    await act(async () => {
      node.dispatchEvent(new WheelEvent("wheel", { deltaY: -400, bubbles: true }));
      node.scrollTop = 300; node.dispatchEvent(new Event("scroll"));
      headings[0].dispatchEvent(new Event("pointerdown", { bubbles: true }));
      const toggled = new Promise<void>(resolve => headings[0].parentElement!.addEventListener("toggle", () => resolve(), { once: true }));
      headings[0].click(); height = 1700; resize();
      await toggled;
    });
    expect(node.scrollTop).toBe(300);
  } finally {
    await act(async () => root.unmount()); heights.mockRestore(); sizes.mockRestore(); vi.unstubAllGlobals();
  }
});
