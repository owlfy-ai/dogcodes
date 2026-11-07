import { useEffect, useRef, useState, useSyncExternalStore, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { copyText, readClipboard } from "./clipboard";
import { FloatingPopover } from "./FloatingPopover";
import { InlineError } from "./InlineError";
import { openTerminalLink } from "./terminalLinks";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";
import "./contextMenu.css";

export interface ContextMenuItem {
  label: string;
  /** Beside the label, for a note such as "Default". */
  hint?: string;
  tooltip?: string;
  disabled?: boolean;
  /** The menu stays up while a promise runs, and shows why if it fails. */
  onSelect: () => void | Promise<unknown>;
}
/** Left-out items may be `false`, `null` or `undefined`; stray separators are tidied away. */
export type ContextMenuEntry = ContextMenuItem | "separator" | false | null | undefined;

type Tidy = ContextMenuItem | "separator";
interface OpenMenu { id: number; point: { x: number; y: number }; entries: Tidy[]; container: Element; returnFocus: HTMLElement | null }
type AnyMouseEvent = ReactMouseEvent | MouseEvent;

let current: OpenMenu | null = null, lastId = 0;
const listeners = new Set<() => void>();
const publish = (menu: OpenMenu | null) => { current = menu; listeners.forEach(listener => listener()); };
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };

function tidy(entries: ContextMenuEntry[]): Tidy[] {
  const out: Tidy[] = [];
  for (const entry of entries) {
    if (!entry) continue;
    if (entry !== "separator" || (out.length && out[out.length - 1] !== "separator")) out.push(entry);
  }
  if (out[out.length - 1] === "separator") out.pop();
  return out;
}

const claimed = new WeakSet<Event>();
/** This event is taken care of, whether or not a menu comes of it: nothing further up
 * should offer the menu for text. */
export function claimContextMenu(event: AnyMouseEvent) {
  event.preventDefault();
  claimed.add("nativeEvent" in event ? event.nativeEvent : event);
}

/** Opens the menu for a right click. Modal dialogs make the rest of the page inert, so a
 * click inside one keeps its menu inside it. */
export function showContextMenu(event: AnyMouseEvent, entries: ContextMenuEntry[]) {
  claimContextMenu(event);
  const items = tidy(entries);
  if (!items.length) return;
  const target = event.target instanceof Element ? event.target : null;
  const active = document.activeElement;
  publish({
    id: ++lastId, point: { x: event.clientX, y: event.clientY }, entries: items,
    container: target?.closest("dialog") ?? document.body,
    returnFocus: active instanceof HTMLElement && active !== document.body ? active : null,
  });
}

/** Moves on to more choices in the same place, such as the apps a file opens with. */
export function replaceContextMenu(entries: ContextMenuEntry[]) {
  if (current) publish({ ...current, id: ++lastId, entries: tidy(entries) });
}

/** Arrow keys, Home and End in a menu. */
export function moveMenuFocus(event: ReactKeyboardEvent<HTMLElement>, selector = '[role="menuitem"]:not(:disabled)') {
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  const items = [...event.currentTarget.querySelectorAll<HTMLElement>(selector)];
  const index = items.indexOf(document.activeElement as HTMLElement);
  const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
  items[next]?.focus();
}

const TEXT_INPUTS = new Set(["", "text", "search", "url", "tel", "password"]);
type TextField = HTMLInputElement | HTMLTextAreaElement;

function textFieldAt(target: Element): TextField | null {
  const field = target.closest<TextField>("input, textarea");
  if (!field || field.disabled) return null;
  return field instanceof HTMLTextAreaElement || TEXT_INPUTS.has(field.getAttribute("type")?.toLowerCase() ?? "") ? field : null;
}

/** Through the editor where it can, so Undo still works and the field hears of it. */
function insertText(field: TextField, text: string) {
  field.focus();
  if (document.execCommand?.("insertText", false, text)) return;
  field.setRangeText(text, field.selectionStart ?? field.value.length, field.selectionEnd ?? field.value.length, "end");
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Tells the page about a paste the way the browser would, so what handles attachments
 * handles this too. Returns whether something took it. */
function announcePaste(field: TextField, text: string, files: File[]) {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: { files, getData: (type: string) => type === "text/plain" ? text : "" } });
  field.dispatchEvent(event);
  return event.defaultPrevented;
}

function fieldMenu(field: TextField): ContextMenuEntry[] {
  const start = field.selectionStart ?? 0, end = field.selectionEnd ?? 0;
  const selected = field.value.slice(start, end);
  const hidden = field instanceof HTMLInputElement && field.type === "password";
  const restore = () => { field.focus(); field.setSelectionRange(start, end); };
  return [
    { label: t("contextMenu.cut"), disabled: !selected || field.readOnly || hidden, onSelect: async () => {
      restore();
      await copyText(selected);
      insertText(field, "");
    } },
    { label: t("contextMenu.copy"), disabled: !selected || hidden, onSelect: () => { restore(); return copyText(selected); } },
    { label: t("contextMenu.paste"), disabled: field.readOnly, onSelect: async () => {
      restore();
      const { text, image } = await readClipboard(!!field.closest("[data-accepts-pasted-files]"));
      if (!text && !image) return;
      field.focus();
      if (!announcePaste(field, text ?? "", image ? [image] : []) && text) insertText(field, text);
    } },
    "separator",
    { label: t("contextMenu.selectAll"), disabled: !field.value, onSelect: () => { field.focus(); field.select(); } },
  ];
}

function selectedTextAt(target: Element) {
  const selection = window.getSelection();
  if (!selection?.rangeCount || selection.isCollapsed || !selection.getRangeAt(0).intersectsNode(target)) return "";
  return selection.toString();
}

/** What every part of the app offers for text, unless something more specific answered.
 * `null` where there's nothing to do: controls and empty space have no menu. */
export function textContextMenu(target: Element): ContextMenuEntry[] | null {
  if (target.closest(".xterm")) return null;
  const field = textFieldAt(target);
  if (field) return fieldMenu(field);
  const href = target.closest("a[href]")?.getAttribute("href");
  const link = href && /^https?:\/\//i.test(href) ? href : null;
  if (!link && target.closest("button, [role='button'], summary, select, a")) return null;
  const selected = selectedTextAt(target);
  const copy: ContextMenuEntry = selected ? { label: t("contextMenu.copy"), onSelect: () => copyText(selected) } : null;
  if (!link) return copy ? [copy] : null;
  return [
    { label: t("contextMenu.openLink"), onSelect: () => openTerminalLink(link) },
    { label: t("contextMenu.copyLink"), onSelect: () => copyText(link) },
    copy && "separator",
    copy,
  ];
}

function Menu({ menu }: { menu: OpenMenu }) {
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  const host = useRef<HTMLDivElement>(null);
  const close = () => { if (current?.id === menu.id) publish(null); };
  const restoreFocus = () => { if (menu.returnFocus?.isConnected) menu.returnFocus.focus(); };
  useEffect(() => {
    host.current?.querySelector<HTMLElement>('[role="menuitem"]:not(:disabled)')?.focus({ preventScroll: true });
    const outside = (event: PointerEvent) => { if (!host.current?.contains(event.target as Node)) close(); };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault(); event.stopPropagation();
      restoreFocus(); close();
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, []);
  const choose = (item: ContextMenuItem) => {
    if (busy) return;
    // Before the action, which may want the focus itself: a dialog, or a field to edit.
    restoreFocus();
    setError("");
    const fail = (cause: unknown) => { setBusy(false); setError(userFacingError(cause, t("contextMenu.failed"))); };
    try {
      const running = item.onSelect();
      if (!running) { close(); return; }
      setBusy(true);
      void running.then(close, fail);
    } catch (cause) { fail(cause); }
  };
  return createPortal(<div ref={host}>
    <FloatingPopover className="context-menu" role="menu" aria-label={t("contextMenu.label")} point={menu.point} onKeyDown={moveMenuFocus} onContextMenu={claimContextMenu}>
      {menu.entries.map((entry, index) => entry === "separator"
        ? <hr key={index} role="separator" />
        : <button key={index} type="button" role="menuitem" disabled={busy || entry.disabled} data-tooltip={entry.tooltip} onClick={() => choose(entry)}><span>{entry.label}</span>{entry.hint && <small>{entry.hint}</small>}</button>)}
      {error && <InlineError>{error}</InlineError>}
    </FloatingPopover>
  </div>, menu.container);
}

/** One in each window, beside `<Tooltips />`. */
export function ContextMenuHost() {
  const menu = useSyncExternalStore(subscribe, () => current);
  useEffect(() => {
    const opened = (event: MouseEvent) => {
      if (claimed.has(event) || !(event.target instanceof Element)) return;
      const entries = textContextMenu(event.target);
      if (entries) showContextMenu(event, entries);
    };
    // Lets the keyboard reach every menu in the app the same way a right click does.
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !(event.key === "ContextMenu" || event.key === "F10" && event.shiftKey)) return;
      const element = document.activeElement;
      if (current || !(element instanceof HTMLElement) || element === document.body) return;
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      element.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left + 12, clientY: rect.bottom }));
    };
    document.addEventListener("contextmenu", opened);
    document.addEventListener("keydown", key);
    return () => { document.removeEventListener("contextmenu", opened); document.removeEventListener("keydown", key); };
  }, []);
  return menu && <Menu key={menu.id} menu={menu} />;
}
