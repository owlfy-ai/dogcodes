// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { ReviewResizeHandle, reviewWidthLimit, useReviewWidth } from "./ReviewResizeHandle";

it("reserves space for the conversation, clamps resizing and remembers the chosen width", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("innerWidth", 1200);
  localStorage.removeItem("dogcodes.review.width");
  const host = document.createElement("div"), root = createRoot(host);
  function Harness() { const state = useReviewWidth(); return <ReviewResizeHandle width={state.width} max={state.max} onResize={state.setWidth} />; }
  try {
    await act(async () => root.render(<Harness />));
    const handle = host.querySelector<HTMLElement>('[role="separator"]')!;
    expect(handle.getAttribute("aria-valuenow")).toBe("380");
    await act(async () => handle.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true })));
    expect(handle.getAttribute("aria-valuenow")).toBe("660");
    expect(localStorage.getItem("dogcodes.review.width")).toBe("660");
    vi.stubGlobal("innerWidth", 1040);
    await act(async () => window.dispatchEvent(new Event("resize")));
    expect(handle.getAttribute("aria-valuenow")).toBe("500");
    await act(async () => handle.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })));
    expect(handle.getAttribute("aria-valuenow")).toBe("380");
    expect(reviewWidthLimit(2000)).toBe(900);
  } finally { await act(async () => root.unmount()); localStorage.removeItem("dogcodes.review.width"); vi.unstubAllGlobals(); }
});

it("resizes from the pointer origin and stops after release", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const host = document.createElement("div"), root = createRoot(host), resize = vi.fn();
  try {
    await act(async () => root.render(<ReviewResizeHandle width={420} max={900} onResize={resize} />));
    const handle = host.querySelector<HTMLElement>('[role="separator"]')!;
    handle.setPointerCapture = vi.fn(); handle.releasePointerCapture = vi.fn();
    const pointer = (type: string, x: number) => { const event = new MouseEvent(type, { bubbles: true, clientX: x, button: 0 }); Object.defineProperty(event, "pointerId", { value: 1 }); handle.dispatchEvent(event); };
    await act(async () => { pointer("pointerdown", 700); pointer("pointermove", 600); });
    expect(resize).toHaveBeenLastCalledWith(520);
    await act(async () => { pointer("pointerup", 600); pointer("pointermove", 500); });
    expect(resize).toHaveBeenCalledTimes(1);
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});
