import { type KeyboardEvent, type ReactNode, useEffect, useRef } from "react";
import { showContextMenu, type ContextMenuEntry } from "./ContextMenu";
import "./tabBar.css";

export interface TabBarItem {
  id: string;
  label: string;
  /** Beside the label, or in its place with `iconOnly`. */
  icon?: ReactNode;
  tooltip?: string;
  /** Left out for a tab that can't be closed. */
  close?: { label: string; tooltip?: string; disabled?: boolean; busy?: boolean; onClose: () => void };
  /** What a right click on this tab offers. */
  menu?: () => ContextMenuEntry[];
}

/** Every row of tabs in the app, so they look and respond alike. Pass no `selected`
 * when the current view is none of the tabs. */
export function TabBar({ label, items, selected, onSelect, iconOnly = false, className, revealKey }: {
  label: string; items: TabBarItem[]; selected?: string | null; onSelect: (id: string) => void; iconOnly?: boolean; className?: string;
  /** Scrolls the selected tab into view again when this changes, e.g. when the bar is shown. */
  revealKey?: unknown;
}) {
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [selected, revealKey]);
  const focusable = items.some(item => item.id === selected) ? selected : items[0]?.id;
  const move = (event: KeyboardEvent, index: number) => {
    const count = items.length;
    const next = event.key === "ArrowRight" ? (index + 1) % count
      : event.key === "ArrowLeft" ? (index + count - 1) % count
      : event.key === "Home" ? 0
      : event.key === "End" ? count - 1
      : -1;
    if (next < 0) return;
    event.preventDefault();
    onSelect(items[next]!.id);
    list.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
  };
  return <div ref={list} className={["tab-bar", iconOnly && "tab-bar--icons", className].filter(Boolean).join(" ")} role="tablist" aria-label={label}>
    {items.map((item, index) => <div className="tab-bar-tab" role="presentation" key={item.id} data-selected={item.id === selected} onContextMenu={item.menu && (event => showContextMenu(event, item.menu!()))}>
      <button type="button" role="tab" aria-selected={item.id === selected} tabIndex={item.id === focusable ? 0 : -1}
        aria-label={iconOnly ? item.label : undefined} data-tooltip={item.tooltip ?? (iconOnly ? item.label : undefined)}
        onClick={() => onSelect(item.id)} onKeyDown={event => move(event, index)}>
        {item.icon}
        {!iconOnly && <span className="tab-bar-label">{item.label}</span>}
      </button>
      {item.close && <button type="button" className="tab-bar-close" aria-label={item.close.label} data-tooltip={item.close.tooltip ?? item.close.label}
        disabled={item.close.disabled} aria-busy={item.close.busy} onClick={item.close.onClose}><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" /></svg></button>}
    </div>)}
  </div>;
}
