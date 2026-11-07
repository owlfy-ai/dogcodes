// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ContextMenuHost, showContextMenu } from "./ContextMenu";

const { invoke, isTauri } = vi.hoisted(() => ({ invoke: vi.fn(), isTauri: vi.fn(() => false) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri }));

let root: Root, container: HTMLDivElement;
const writeText = vi.fn(async (_: string) => {}), readText = vi.fn(async () => "pasted");
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText, readText } });
  writeText.mockClear(); invoke.mockReset(); isTauri.mockReturnValue(false);
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => {
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  await act(async () => root.unmount()); container.remove(); window.getSelection()?.removeAllRanges(); vi.unstubAllGlobals();
});

const items = () => [...document.querySelectorAll<HTMLButtonElement>('.context-menu [role="menuitem"]')];
const labels = () => items().map(item => item.textContent);
const rightClick = (target: Element, init: MouseEventInit = {}) => act(async () => { target.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 5, clientY: 5, ...init })); });
const choose = (label: string) => act(async () => items().find(item => item.textContent === label)!.click());
const render = (children: React.ReactNode) => act(async () => root.render(<><ContextMenuHost />{children}</>));

function Field({ onPaste }: { onPaste?: (event: React.ClipboardEvent) => void }) {
  const [value, setValue] = useState("hello world");
  return <div onPaste={onPaste} data-accepts-pasted-files=""><textarea aria-label="field" value={value} onChange={event => setValue(event.target.value)} /></div>;
}

it("gives a text field cut, copy, paste and select all, reflecting its selection", async () => {
  await render(<Field />);
  const field = container.querySelector("textarea")!;
  field.focus(); field.setSelectionRange(0, 0);
  await rightClick(field);
  expect(labels()).toEqual(["剪切", "复制", "粘贴", "全选"]);
  expect(items().map(item => item.disabled)).toEqual([true, true, false, false]);
  await choose("全选");
  expect([field.selectionStart, field.selectionEnd]).toEqual([0, 11]);
  field.setSelectionRange(0, 5);
  await rightClick(field);
  await choose("复制");
  expect(writeText).toHaveBeenCalledWith("hello");
  await rightClick(field);
  await choose("剪切");
  expect(field.value).toBe(" world");
  expect(writeText).toHaveBeenLastCalledWith("hello");
  field.setSelectionRange(0, 0);
  await rightClick(field);
  await choose("粘贴");
  expect(field.value).toBe("pasted world");
});

it("keeps a password out of the clipboard and doesn't edit a read-only field", async () => {
  await render(<><input type="password" defaultValue="secret" /><input readOnly defaultValue="fixed" /><input type="checkbox" /></>);
  const [password, readOnly, checkbox] = container.querySelectorAll("input");
  password!.select();
  await rightClick(password!);
  expect(items().slice(0, 2).map(item => item.disabled)).toEqual([true, true]);
  readOnly!.select();
  await rightClick(readOnly!);
  expect(items().map(item => item.disabled)).toEqual([true, false, true, false]);
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  await rightClick(checkbox!);
  expect(items()).toHaveLength(0);
});

it("reads the clipboard natively in the app and hands an image to what takes attachments", async () => {
  isTauri.mockReturnValue(true);
  invoke.mockResolvedValue({ text: null, image: btoa("png") });
  const pasted: File[] = [];
  await render(<Field onPaste={event => { pasted.push(...Array.from(event.clipboardData.files)); event.preventDefault(); }} />);
  const field = container.querySelector("textarea")!;
  await rightClick(field);
  await choose("粘贴");
  expect(invoke).toHaveBeenCalledWith("read_clipboard", { includeImage: true });
  expect(pasted.map(file => [file.name, file.type])).toEqual([["clipboard.png", "image/png"]]);
  expect(field.value).toBe("hello world");
});

it("offers Copy only for the selection under the pointer, and nothing on controls or empty space", async () => {
  await render(<><p id="message">Some text</p><p id="other">Other</p><button type="button">Send</button></>);
  const message = container.querySelector("#message")!;
  await rightClick(message);
  expect(items()).toHaveLength(0);
  window.getSelection()!.selectAllChildren(message);
  await rightClick(container.querySelector("#other")!);
  expect(items()).toHaveLength(0);
  await rightClick(container.querySelector("button")!);
  expect(items()).toHaveLength(0);
  await rightClick(message);
  expect(labels()).toEqual(["复制"]);
  await choose("复制");
  expect(writeText).toHaveBeenCalledWith("Some text");
  expect(items()).toHaveLength(0);
});

it("offers opening and copying a web link, and nothing for other kinds", async () => {
  const open = vi.fn(); vi.stubGlobal("open", open);
  await render(<><a href="https://example.com/a">link</a><a href="/local">local</a></>);
  const [web, local] = container.querySelectorAll("a");
  await rightClick(local!);
  expect(items()).toHaveLength(0);
  await rightClick(web!);
  expect(labels()).toEqual(["打开链接", "复制链接"]);
  await choose("复制链接");
  expect(writeText).toHaveBeenCalledWith("https://example.com/a");
  await rightClick(web!);
  await choose("打开链接");
  expect(open).toHaveBeenCalledWith("https://example.com/a", "_blank", "noopener");
});

it("lets an object's own menu win, tidies separators and shows why an action failed", async () => {
  const fails = vi.fn(async () => { throw new Error("nope"); });
  await render(<div id="row" onContextMenu={event => showContextMenu(event, ["separator", { label: "One", onSelect: fails }, false, "separator", "separator", { label: "Two", disabled: true, onSelect() {} }, "separator"])}><input defaultValue="inner" /></div>);
  await rightClick(container.querySelector("input")!);
  expect(labels()).toEqual(["One", "Two"]);
  expect(document.querySelectorAll(".context-menu hr")).toHaveLength(1);
  await choose("One");
  expect(items()).toHaveLength(2);
  expect(document.querySelector(".context-menu")!.textContent).toContain("操作没有成功");
});

it("closes on Escape or a click elsewhere, returns focus, and moves with the arrow keys", async () => {
  await render(<><button type="button" id="row" onContextMenu={event => showContextMenu(event, [{ label: "One", onSelect() {} }, { label: "Off", disabled: true, onSelect() {} }, { label: "Two", onSelect() {} }])}>row</button><p id="out">out</p></>);
  const row = container.querySelector<HTMLButtonElement>("#row")!;
  row.focus();
  await rightClick(row);
  expect(document.activeElement?.textContent).toBe("One");
  const menu = document.querySelector(".context-menu")!;
  await act(async () => menu.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })));
  expect(document.activeElement?.textContent).toBe("Two");
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(items()).toHaveLength(0);
  expect(document.activeElement).toBe(row);
  await rightClick(row);
  await act(async () => container.querySelector("#out")!.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true })));
  expect(items()).toHaveLength(0);
});

it("opens from the keyboard, and inside a modal dialog stays usable there", async () => {
  await render(<><button type="button" id="row" onContextMenu={event => showContextMenu(event, [{ label: "One", onSelect() {} }])}>row</button><dialog open><input defaultValue="in dialog" /></dialog></>);
  const row = container.querySelector<HTMLButtonElement>("#row")!;
  row.focus();
  await act(async () => row.dispatchEvent(new KeyboardEvent("keydown", { key: "F10", shiftKey: true, bubbles: true })));
  expect(labels()).toEqual(["One"]);
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  await rightClick(container.querySelector("dialog input")!);
  expect(document.querySelector("dialog .context-menu")).not.toBeNull();
});
