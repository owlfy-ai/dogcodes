// @vitest-environment jsdom
import { act, useLayoutEffect } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { ConversationScroll } from "./ConversationScroll";

it.each(["wheel", "scrollbar", "native scrollbar", "native scrollbar before resize"])("keeps following paused after a small upward %s movement during streaming", async input => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let resize!: () => void;
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { resize = callback; }
    observe() {} disconnect() {}
  });
  let height = 1400, top = 0;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  // Model the browser's clamping; assigning scrollHeight lands at height - clientHeight.
  const tops = vi.spyOn(HTMLElement.prototype, "scrollTop", "get").mockImplementation(() => top);
  const writes = vi.spyOn(HTMLElement.prototype, "scrollTop", "set").mockImplementation(value => { top = Math.max(0, Math.min(value, height - 500)); });
  const host = document.createElement("div"), root = createRoot(host);
  try {
    await act(async () => root.render(<ConversationScroll taskId="one"><p>Streaming reply</p></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    expect(node.scrollTop).toBe(900);
    await act(async () => {
      if (input === "wheel") node.dispatchEvent(new WheelEvent("wheel", { deltaY: -40, bubbles: true }));
      if (input === "scrollbar") node.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      // A queued event from the preceding automatic scroll must not re-enable follow.
      node.dispatchEvent(new Event("scroll"));
      node.scrollTop = 860;
      if (input === "native scrollbar before resize") resize();
      node.dispatchEvent(new Event("scroll"));
      height = 1420;
      resize();
    });
    expect(node.scrollTop).toBe(860);
    expect(host.textContent).toContain("回到底部");

    // Moving down within the old 80px threshold still leaves the reader in control.
    await act(async () => {
      node.scrollTop = 890;
      node.dispatchEvent(new Event("scroll"));
      height = 1440;
      resize();
    });
    expect(node.scrollTop).toBe(890);
    await act(async () => {
      node.scrollTop = 940;
      node.dispatchEvent(new Event("scroll"));
      height = 1500;
      resize();
    });
    expect(node.scrollTop).toBe(1000);

    await act(async () => {
      node.scrollTop = 960;
      node.dispatchEvent(new Event("scroll"));
    });
    await act(async () => {
      host.querySelector<HTMLButtonElement>(".conversation-latest")!.click();
      height = 1600;
      resize();
    });
    expect(node.scrollTop).toBe(1100);
    expect(host.textContent).not.toContain("回到底部");
  } finally {
    await act(async () => root.unmount());
    heights.mockRestore(); sizes.mockRestore(); tops.mockRestore(); writes.mockRestore(); vi.unstubAllGlobals();
  }
});

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

it.each(["scroll first", "resize first"])("keeps following through a streaming layout shrink and regrowth (%s)", async order => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let resize!: () => void;
  vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect() {} });
  let height = 1400, top = 0;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const tops = vi.spyOn(HTMLElement.prototype, "scrollTop", "get").mockImplementation(() => top);
  const writes = vi.spyOn(HTMLElement.prototype, "scrollTop", "set").mockImplementation(value => { top = Math.max(0, Math.min(value, height - 500)); });
  const host = document.createElement("div"), root = createRoot(host);
  try {
    await act(async () => root.render(<ConversationScroll taskId="one"><p>Streaming reply</p></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    expect(top).toBe(900);
    await act(async () => {
      // Markdown/tool output can briefly shrink and clamp the native scrollTop,
      // then grow again before the queued scroll or ResizeObserver is delivered.
      height = 1000; node.scrollTop = top;
      height = 1600;
      if (order === "resize first") resize();
      node.dispatchEvent(new Event("scroll"));
      if (order === "scroll first") resize();
    });
    expect(top).toBe(1100);
    expect(host.querySelector(".conversation-latest")).toBeNull();
    await act(async () => { height = 1700; resize(); });
    expect(top).toBe(1200);
  } finally {
    await act(async () => root.unmount());
    heights.mockRestore(); sizes.mockRestore(); tops.mockRestore(); writes.mockRestore(); vi.unstubAllGlobals();
  }
});

it("keeps following when a re-render clamps the position and restores the same height", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  let height = 1400, top = 0;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const tops = vi.spyOn(HTMLElement.prototype, "scrollTop", "get").mockImplementation(() => top);
  const writes = vi.spyOn(HTMLElement.prototype, "scrollTop", "set").mockImplementation(value => { top = Math.max(0, Math.min(value, height - 500)); });
  const host = document.createElement("div"), root = createRoot(host);
  try {
    await act(async () => root.render(<ConversationScroll taskId="one"><p key="turn">Turn</p></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    expect(top).toBe(900);
    await act(async () => {
      // The turn's actions mount: its old DOM leaves first and a layout read in
      // that window clamps scrollTop. No resize is observable afterwards.
      height = 800; node.scrollTop = top; height = 1400;
      root.render(<ConversationScroll taskId="one"><p key="settled">Turn</p></ConversationScroll>);
    });
    await act(async () => { node.dispatchEvent(new Event("scroll")); });
    expect(top).toBe(900);
    expect(host.querySelector(".conversation-latest")).toBeNull();
  } finally {
    await act(async () => root.unmount());
    heights.mockRestore(); sizes.mockRestore(); tops.mockRestore(); writes.mockRestore(); vi.unstubAllGlobals();
  }
});

it("does not resume following when content changes after scrolling up", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  let height = 1400, top = 0;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const tops = vi.spyOn(HTMLElement.prototype, "scrollTop", "get").mockImplementation(() => top);
  const writes = vi.spyOn(HTMLElement.prototype, "scrollTop", "set").mockImplementation(value => { top = Math.max(0, Math.min(value, height - 500)); });
  const host = document.createElement("div"), root = createRoot(host);
  try {
    await act(async () => root.render(<ConversationScroll taskId="one"><p key="turn">Turn</p></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    await act(async () => {
      node.dispatchEvent(new WheelEvent("wheel", { deltaY: -40, bubbles: true }));
      node.scrollTop = 600;
      node.dispatchEvent(new Event("scroll"));
    });
    await act(async () => root.render(<ConversationScroll taskId="one"><p key="settled">Turn</p></ConversationScroll>));
    expect(top).toBe(600);
    expect(host.querySelector(".conversation-latest")).not.toBeNull();
  } finally {
    await act(async () => root.unmount());
    heights.mockRestore(); sizes.mockRestore(); tops.mockRestore(); writes.mockRestore(); vi.unstubAllGlobals();
  }
});

it("keeps following when viewport resizing clamps the position during streaming", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let resize!: () => void;
  vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect() {} });
  let height = 1400, size = 500, top = 0;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockImplementation(() => size);
  const tops = vi.spyOn(HTMLElement.prototype, "scrollTop", "get").mockImplementation(() => top);
  const writes = vi.spyOn(HTMLElement.prototype, "scrollTop", "set").mockImplementation(value => { top = Math.max(0, Math.min(value, height - size)); });
  const host = document.createElement("div"), root = createRoot(host);
  try {
    await act(async () => root.render(<ConversationScroll taskId="one"><p>Streaming reply</p></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    await act(async () => {
      size = 700; node.scrollTop = top;
      height = 1500;
      node.dispatchEvent(new Event("scroll")); resize();
    });
    expect(top).toBe(800);
    expect(host.querySelector(".conversation-latest")).toBeNull();
  } finally {
    await act(async () => root.unmount());
    heights.mockRestore(); sizes.mockRestore(); tops.mockRestore(); writes.mockRestore(); vi.unstubAllGlobals();
  }
});

it.each(["pointerdown", "touchstart"])("does not pause following for a stationary %s without a scroll", async input => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let resize!: () => void;
  vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect() {} });
  let height = 1400;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const host = document.createElement("div"), root = createRoot(host);
  try {
    await act(async () => root.render(<ConversationScroll taskId="one"><p>Streaming reply</p></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    await act(async () => {
      node.querySelector("p")!.dispatchEvent(new Event(input, { bubbles: true }));
      height = 1600; resize();
    });
    expect(node.scrollTop).toBe(1600);
    expect(host.querySelector(".conversation-latest")).toBeNull();
  } finally {
    await act(async () => root.unmount()); heights.mockRestore(); sizes.mockRestore(); vi.unstubAllGlobals();
  }
});

it.each(["keyboard", "touch"])("preserves explicit upward %s input while content resizes", async input => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let resize!: () => void;
  vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect() {} });
  let height = 1400, top = 0;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const tops = vi.spyOn(HTMLElement.prototype, "scrollTop", "get").mockImplementation(() => top);
  const writes = vi.spyOn(HTMLElement.prototype, "scrollTop", "set").mockImplementation(value => { top = Math.max(0, Math.min(value, height - 500)); });
  const host = document.createElement("div"), root = createRoot(host);
  try {
    await act(async () => root.render(<ConversationScroll taskId="one"><p>Streaming reply</p></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    await act(async () => {
      if (input === "keyboard") node.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
      else for (const [type, y] of [["touchstart", 100], ["touchmove", 140]] as const) {
        const event = new Event(type, { bubbles: true });
        Object.defineProperty(event, "touches", { value: [{ clientY: y }] });
        node.dispatchEvent(event);
      }
      node.scrollTop = 860;
      height = 1600; resize(); node.dispatchEvent(new Event("scroll"));
    });
    expect(top).toBe(860);
    expect(host.querySelector(".conversation-latest")).not.toBeNull();
  } finally {
    await act(async () => root.unmount());
    heights.mockRestore(); sizes.mockRestore(); tops.mockRestore(); writes.mockRestore(); vi.unstubAllGlobals();
  }
});

it("does not treat a message button's keyboard activation as scrolling history", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let resize!: () => void;
  vi.stubGlobal("ResizeObserver", class { constructor(callback: () => void) { resize = callback; } observe() {} disconnect() {} });
  let height = 1400;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const host = document.createElement("div"), root = createRoot(host);
  try {
    await act(async () => root.render(<ConversationScroll taskId="one"><button>Copy</button></ConversationScroll>));
    const node = host.querySelector<HTMLElement>(".timeline")!;
    await act(async () => {
      node.querySelector("button")!.dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true }));
      height = 1600; resize();
    });
    expect(node.scrollTop).toBe(1600);
    expect(host.querySelector(".conversation-latest")).toBeNull();
  } finally {
    await act(async () => root.unmount()); heights.mockRestore(); sizes.mockRestore(); vi.unstubAllGlobals();
  }
});
