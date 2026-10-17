// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ReviewFileTree } from "./ReviewFileTree";
import { useReviewFileSelection } from "./useReviewFileSelection";
import type { ReviewSnapshot } from "./reviewClient";
let root: Root, container: HTMLDivElement;
const preview = vi.fn();
const snapshot: ReviewSnapshot = { id: "s", taskId: "task", roots: ["one", "two"].map(id => ({ root: { id, path: `/${id}`, git: true, scopes: [""] }, mode: "unstaged", base: "", target: "", error: null, untracked: [], statistics: {}, files: ["z.ts", "dir/b.ts", "dir/a.ts"].map(path => ({ path, kind: "modified", before: null, after: null })) })) };
function Harness({ scope = "one", data = snapshot, disabled = false }: { scope?: string; data?: ReviewSnapshot; disabled?: boolean }) {
  const select = useReviewFileSelection(data, scope, disabled);
  return <nav ref={select.nav} tabIndex={0} onPointerDown={select.onPointerDown} onClickCapture={select.onClickCapture} onKeyDown={select.onKeyDown}>
    {data.roots.map(result => <ReviewFileTree key={result.root.id} result={result} filter="" busy={disabled} selectedFiles={select.selected} onSelect={preview} />)}
    {select.rectangle && <div data-rectangle style={select.rectangle} />}
  </nav>;
}
const render = (props = {}) => act(async () => root.render(<Harness {...props} />));
const rows = () => [...container.querySelectorAll<HTMLButtonElement>(".review-file-entry")];
const selected = () => rows().filter(row => row.getAttribute("aria-pressed") === "true");
const click = (element: Element, init: MouseEventInit = {}) => act(async () => { element.dispatchEvent(new MouseEvent("click", { bubbles: true, ...init })); });
const key = (value: string, init: KeyboardEventInit = {}) => act(async () => { container.querySelector("nav")!.dispatchEvent(new KeyboardEvent("keydown", { key: value, bubbles: true, ...init })); });
const pointer = (element: EventTarget, type: string, x: number, y: number, init: MouseEventInit = {}) => act(async () => {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientX: x, clientY: y, ...init });
  Object.defineProperties(event, { pointerId: { value: 1 }, pointerType: { value: "mouse" } });
  element.dispatchEvent(event);
});
function layout() {
  const nav = container.querySelector("nav")!;
  vi.spyOn(nav, "getBoundingClientRect").mockReturnValue({ left: 0, top: 0, width: 200, height: 240, bottom: 240, right: 200 } as DOMRect);
  Object.defineProperties(nav, { clientWidth: { value: 200 }, clientHeight: { value: 240 } });
  rows().forEach((row, i) => vi.spyOn(row, "getBoundingClientRect").mockReturnValue({ left: 10, top: 10 + i * 30, width: 180, height: 28, right: 190, bottom: 38 + i * 30 } as DOMRect));
  return nav;
}
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); preview.mockReset(); container = document.createElement("div"); document.body.append(container); root = createRoot(container); });
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it("uses visible sorted order for Shift ranges across roots without changing the preview", async () => {
  await render(); await click(rows()[1]); await click(rows()[4], { shiftKey: true });
  expect(selected()).toEqual(rows().slice(1, 5)); expect(preview).toHaveBeenCalledTimes(1);
  await click(rows()[2], { shiftKey: true }); expect(selected()).toEqual(rows().slice(1, 3));
});
it("toggles with both Ctrl and Cmd, combines ranges, then resets on a plain click", async () => {
  await render(); await click(rows()[0]); await click(rows()[3], { ctrlKey: true }); await click(rows()[5], { metaKey: true, shiftKey: true });
  expect(selected()).toEqual([rows()[0], ...rows().slice(3)]);
  await click(rows()[0], { metaKey: true }); expect(selected()).toEqual(rows().slice(3));
  await click(rows()[1]); expect(selected()).toEqual([rows()[1]]);
});
it("excludes collapsed tree files from range and select all; Escape clears selection", async () => {
  await render(); await click(container.querySelector('[data-root-id="one"] [data-tooltip="dir"]')!);
  await click(rows()[0]); await click(rows()[2], { shiftKey: true }); expect(selected()).toEqual(rows().slice(0, 3));
  await key("a", { metaKey: true }); expect(selected()).toEqual(rows()); await key("Escape"); expect(selected()).toHaveLength(0);
});
it("prunes disappeared files on refresh and clears selection when comparison scope changes", async () => {
  await render(); await key("a", { ctrlKey: true });
  await render({ data: { ...snapshot, roots: snapshot.roots.slice(0, 1) } }); expect(selected()).toHaveLength(3);
  await render({ scope: "next" }); expect(selected()).toHaveLength(0);
});
it("draws a rectangle from a file, selects only intersected rows, and suppresses its trailing click", async () => {
  await render(); layout(); await pointer(rows()[0], "pointerdown", 20, 20); await pointer(window, "pointermove", 170, 93);
  expect(selected()).toEqual(rows().slice(0, 3)); expect(container.querySelector("[data-rectangle]")).not.toBeNull();
  // Pointerup and click are in the same native event turn.
  await act(async () => {
    const event = new MouseEvent("pointerup", { bubbles: true }); Object.defineProperty(event, "pointerId", { value: 1 }); window.dispatchEvent(event);
    rows()[2].click();
  });
  expect(preview).not.toHaveBeenCalled(); expect(selected()).toHaveLength(3); expect(container.querySelector("[data-rectangle]")).toBeNull();
});
it("supports additive rectangles from whitespace and restores the prior selection on cancellation", async () => {
  await render(); const nav = layout(); await click(rows()[5]);
  await pointer(nav, "pointerdown", 2, 2, { ctrlKey: true }); await pointer(window, "pointermove", 100, 63);
  expect(selected()).toEqual([rows()[0], rows()[1], rows()[5]]);
  await pointer(window, "pointercancel", 100, 63); expect(selected()).toEqual([rows()[5]]);
});
it("does not turn a small pointer movement into a drag or select while disabled", async () => {
  await render(); layout(); await pointer(rows()[0], "pointerdown", 20, 20); await pointer(window, "pointermove", 22, 21); await pointer(window, "pointerup", 22, 21); await click(rows()[0]);
  expect(selected()).toHaveLength(1); expect(preview).toHaveBeenCalledTimes(1);
  await render({ disabled: true }); await key("a", { ctrlKey: true }); expect(selected()).toHaveLength(1);
});

it("scrolls at the list edge during a rectangle and keeps newly reached files selected", async () => {
  let tick: FrameRequestCallback = () => {};
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { tick = callback; return 1; });
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  await render(); const nav = layout();
  rows().forEach((row, i) => vi.spyOn(row, "getBoundingClientRect").mockImplementation(() => ({ left: 10, top: 10 + i * 50 - nav.scrollTop, width: 180, height: 28, right: 190, bottom: 38 + i * 50 - nav.scrollTop } as DOMRect)));
  await pointer(nav, "pointerdown", 2, 2); await pointer(window, "pointermove", 100, 239);
  expect(selected()).toHaveLength(5);
  await act(async () => { tick(16); tick(32); tick(48); });
  expect(nav.scrollTop).toBe(30); expect(selected()).toHaveLength(6); expect(document.activeElement).toBe(nav);
  await pointer(window, "pointerup", 100, 239);
});


it.each(["empty", "error", "collapsed"])("does not start a rectangle when the list is %s", async state => {
  const data = state === "empty" ? { ...snapshot, roots: snapshot.roots.map(root => ({ ...root, files: [] })) }
    : state === "error" ? { ...snapshot, roots: snapshot.roots.map(root => ({ ...root, error: "读取失败" })) } : snapshot;
  await render({ data });
  if (state === "collapsed") {
    for (const heading of container.querySelectorAll(".review-root-heading")) await click(heading);
  }
  const nav = layout();
  expect(rows()).toHaveLength(0);
  await pointer(nav, "pointerdown", 20, 20); await pointer(window, "pointermove", 150, 180);
  expect(container.querySelector("[data-rectangle]")).toBeNull();
  expect(preview).not.toHaveBeenCalled();
  await pointer(window, "pointerup", 150, 180);
});

it("removes an active rectangle when a refresh replaces the list with an empty snapshot", async () => {
  await render(); const nav = layout();
  await pointer(nav, "pointerdown", 20, 20); await pointer(window, "pointermove", 150, 90);
  expect(container.querySelector("[data-rectangle]")).not.toBeNull();
  await render({ data: { ...snapshot, id: "empty", roots: snapshot.roots.map(root => ({ ...root, files: [] })) } });
  expect(container.querySelector("[data-rectangle]")).toBeNull();
  await pointer(window, "pointermove", 160, 180);
  expect(container.querySelector("[data-rectangle]")).toBeNull();
  await pointer(window, "pointerup", 160, 180);
});
