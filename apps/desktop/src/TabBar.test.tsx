// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { TabBar } from "./TabBar";

let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});
const render = (element: React.ReactNode) => act(async () => root.render(element));
const tabs = () => [...container.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
const items = [{ id: "a", label: "A" }, { id: "b", label: "B" }, { id: "c", label: "C" }];

it("moves with the arrow keys, wraps around, and focuses the tab it moved to", async () => {
  const onSelect = vi.fn();
  await render(<TabBar label="Tabs" items={items} selected="a" onSelect={onSelect} />);
  tabs()[0]!.focus();
  await act(async () => { tabs()[0]!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true })); });
  expect(onSelect).toHaveBeenLastCalledWith("c");
  expect(document.activeElement).toBe(tabs()[2]);
  await act(async () => { tabs()[2]!.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true })); });
  expect(onSelect).toHaveBeenLastCalledWith("a");
});

it("offers closing only on tabs that can close", async () => {
  const onClose = vi.fn();
  await render(<TabBar label="Tabs" selected="a" onSelect={() => {}} items={[{ id: "a", label: "A" }, { id: "b", label: "B", close: { label: "Close B", onClose } }]} />);
  const close = container.querySelectorAll<HTMLButtonElement>(".tab-bar-close");
  expect(close.length).toBe(1);
  expect(close[0]!.getAttribute("aria-label")).toBe("Close B");
  await act(async () => close[0]!.click());
  expect(onClose).toHaveBeenCalled();
});

it("stays reachable from the keyboard when no tab is selected", async () => {
  await render(<TabBar label="Tabs" items={items} onSelect={() => {}} />);
  expect(tabs().map(tab => tab.tabIndex)).toEqual([0, -1, -1]);
  expect(container.querySelector('[aria-selected="true"]')).toBeNull();
});

it("names an icon tab for assistive technology and in its tooltip", async () => {
  await render(<TabBar iconOnly label="Tabs" selected="a" onSelect={() => {}} items={[{ id: "a", label: "Files", icon: <svg /> }]} />);
  expect(tabs()[0]!.getAttribute("aria-label")).toBe("Files");
  expect(tabs()[0]!.getAttribute("data-tooltip")).toBe("Files");
  expect(tabs()[0]!.textContent).toBe("");
});
