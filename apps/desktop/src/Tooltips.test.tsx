// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Tooltips } from "./Tooltips";
import { installFocusVisibility } from "./focusVisibility";

let root: Root, container: HTMLDivElement;
const title = "审批演示：请使用终端工具";
const size = (element: HTMLElement, visible: number, content: number) => {
  Object.defineProperties(element, { clientWidth: { configurable: true, value: visible }, scrollWidth: { configurable: true, value: content } });
};
const hover = (element: HTMLElement) => act(async () => { element.dispatchEvent(new MouseEvent("pointerover", { bubbles: true })); });
const settle = () => act(async () => { vi.advanceTimersByTime(500); });
const render = (archived = false) => act(async () => root.render(<>
  {archived ? <span className="task-title" data-tooltip={title} data-tooltip-overflow="">{title}</span>
    : <button className="task" data-tooltip={title} data-tooltip-overflow=".task-title"><span className="task-title">{title}</span></button>}
  <button className="rename" data-tooltip="重命名会话">编辑</button><Tooltips />
</>));
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.useFakeTimers();
  vi.spyOn(HTMLElement.prototype, "getClientRects").mockReturnValue([{}] as unknown as DOMRectList);
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it.each([false, true])("shows only truncated titles (archived: %s)", async archived => {
  await render(archived);
  const label = container.querySelector<HTMLElement>(".task-title")!;
  size(label, 200, 200); await hover(label); await settle();
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
  await act(async () => label.dispatchEvent(new MouseEvent("pointerout", { bubbles: true })));
  size(label, 100, 200); await hover(label); await settle();
  expect(document.querySelector('[role="tooltip"]')?.textContent).toBe(title);
});
it("measures the title after hover layout changes and keeps icon tooltips unconditional", async () => {
  await render(); const label = container.querySelector<HTMLElement>(".task-title")!;
  size(label, 200, 200); await hover(label); size(label, 100, 200); await settle();
  expect(document.querySelector('[role="tooltip"]')?.textContent).toBe(title);
  await hover(container.querySelector<HTMLElement>(".rename")!); await settle();
  expect(document.querySelector('[role="tooltip"]')?.textContent).toBe("重命名会话");
});
it("uses the same truncation check for keyboard focus", async () => {
  await render(); const button = container.querySelector<HTMLButtonElement>(".task")!;
  const label = container.querySelector<HTMLElement>(".task-title")!;
  vi.spyOn(button, "matches").mockImplementation(selector => selector === ":focus-visible");
  size(label, 200, 200); await act(async () => button.focus()); await settle();
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
  await act(async () => button.blur()); size(label, 100, 200);
  await act(async () => button.focus()); await settle();
  expect(document.querySelector('[role="tooltip"]')?.textContent).toBe(title);
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
});

it("shows hints on disabled controls even when a child stops bubbling", async () => {
  await act(async () => root.render(<><button disabled data-tooltip="请先选择文件"><svg><path /></svg></button><Tooltips /></>));
  const icon = container.querySelector("path")!;
  icon.addEventListener("pointerover", event => event.stopPropagation());
  await act(async () => icon.dispatchEvent(new MouseEvent("pointerover", { bubbles: true })));
  await settle();
  expect(document.querySelector('[role="tooltip"]')?.textContent).toBe("请先选择文件");
  expect(container.querySelector("button")?.disabled).toBe(true);
});

it("does not reopen a dismissed tooltip when pointer-driven Escape restores focus", async () => {
  const dispose = installFocusVisibility(document);
  try {
    await render();
    const button = container.querySelector<HTMLButtonElement>(".rename")!;
    vi.spyOn(button, "matches").mockImplementation(selector => selector === ":focus-visible");
    await hover(button); await settle();
    expect(document.querySelector('[role="tooltip"]')).not.toBeNull();
    await act(async () => {
      button.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      button.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      button.focus();
    });
    await settle();
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
    expect(document.activeElement).toBe(button);
    await act(async () => {
      button.blur();
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
      button.focus();
    });
    await settle();
    expect(document.querySelector('[role="tooltip"]')?.textContent).toBe("重命名会话");
  } finally { dispose(); }
});

it("updates a visible hint when its text changes and hides it when removed", async () => {
  const renderStatus = (text?: string) => act(async () => root.render(<><button data-tooltip={text}>状态</button><Tooltips /></>));
  await renderStatus("正在检查");
  await hover(container.querySelector("button")!); await settle();
  expect(document.querySelector('[role="tooltip"]')?.textContent).toBe("正在检查");
  await renderStatus("检查失败\n点击重试");
  expect(document.querySelector('[role="tooltip"]')?.textContent).toBe("检查失败\n点击重试");
  await renderStatus(undefined);
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
  expect(container.querySelector("button")?.hasAttribute("aria-describedby")).toBe(false);
});

it("preserves existing accessible descriptions and cleans up after a trigger is removed", async () => {
  await act(async () => root.render(<><button aria-describedby="details" data-tooltip="完整路径">文件</button><p id="details">说明</p><Tooltips /></>));
  const trigger = container.querySelector("button")!;
  await hover(trigger); await settle();
  const tooltip = document.querySelector('[role="tooltip"]')!;
  expect(trigger.getAttribute("aria-describedby")?.split(" ")).toEqual(["details", tooltip.id]);
  await act(async () => trigger.dispatchEvent(new MouseEvent("pointerout", { bubbles: true })));
  expect(trigger.getAttribute("aria-describedby")).toBe("details");
  await hover(trigger); await settle();
  await act(async () => root.render(<Tooltips />));
  expect(document.querySelector('[role="tooltip"]')).toBeNull();
});
