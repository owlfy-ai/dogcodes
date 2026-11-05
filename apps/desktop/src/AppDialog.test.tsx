// @vitest-environment jsdom
import { act, createRef } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { AppDialog, ConfirmDialog, DialogButton } from "./AppDialog";

let root: Root, host: HTMLDivElement;
const dialog = () => document.querySelector("dialog")!;
const render = (content: React.ReactNode) => act(async () => root.render(content));
const text = (label: string) => [...document.querySelectorAll("button")].find(item => item.textContent === label)!;

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });

it("opens outside its parent, labelled by its title and starting on it", async () => {
  await render(<AppDialog title="Heading" onCancel={() => {}} actions={<DialogButton>Close</DialogButton>}><p>Body</p></AppDialog>);
  expect(dialog().open).toBe(true);
  expect(host.contains(dialog())).toBe(false);
  expect(document.getElementById(dialog().getAttribute("aria-labelledby")!)?.textContent).toBe("Heading");
  expect(document.activeElement).toBe(dialog().querySelector("h2"));
});

it("starts on what asks for focus, and returns focus to the trigger afterwards", async () => {
  const trigger = createRef<HTMLButtonElement>();
  const page = (open: boolean) => <>
    <button ref={trigger}>Open</button>
    {open && <ConfirmDialog title="Sure?" confirmLabel="Yes" focusConfirm returnFocus={trigger} onConfirm={() => {}} onCancel={() => {}} />}
  </>;
  await render(page(true));
  expect(document.activeElement).toBe(text("Yes"));
  await render(page(false));
  expect(document.activeElement).toBe(trigger.current);
});

it("does not let Escape or Cancel dismiss a confirmation at work", async () => {
  const cancel = vi.fn(), confirm = vi.fn();
  const page = (busy: boolean) => <ConfirmDialog title="Sure?" confirmLabel="Yes" busy={busy} onConfirm={confirm} onCancel={cancel} />;
  await render(page(true));
  await act(async () => { dialog().dispatchEvent(new Event("cancel", { cancelable: true })); });
  expect(cancel).not.toHaveBeenCalled();
  expect(text("Yes").disabled).toBe(true);
  await render(page(false));
  await act(async () => { dialog().dispatchEvent(new Event("cancel", { cancelable: true })); });
  expect(cancel).toHaveBeenCalledTimes(1);
  await act(async () => text("Yes").click());
  expect(confirm).toHaveBeenCalledTimes(1);
});

it("keeps clicks from reaching the React parent", async () => {
  const parent = vi.fn();
  await render(<div onClick={parent}><ConfirmDialog title="Sure?" confirmLabel="Yes" onConfirm={() => {}} onCancel={() => {}} /></div>);
  await act(async () => text("Yes").click());
  expect(parent).not.toHaveBeenCalled();
});

it("submits a form with Enter when asked", async () => {
  const submit = vi.fn();
  await render(<AppDialog title="Ask" onCancel={() => {}} onSubmit={submit} actions={<DialogButton type="submit">Go</DialogButton>} />);
  await act(async () => dialog().querySelector("form")!.requestSubmit());
  expect(submit).toHaveBeenCalledTimes(1);
});
