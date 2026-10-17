import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import "./tooltips.css";

function detailRows(raw?: string): [string, string][] {
  try {
    const rows: unknown = JSON.parse(raw ?? "null");
    return Array.isArray(rows) ? rows.filter((row): row is [string, string] => Array.isArray(row) && row.length === 2 && row.every(value => typeof value === "string")).slice(0, 20) : [];
  } catch { return []; }
}

/** Shared, fast tooltips for controls marked with data-tooltip. */
export function Tooltips() {
  const [active, setActive] = useState<{ anchor: HTMLElement; text: string; rows?: string } | null>(null);
  const tooltip = useRef<HTMLDivElement>(null);
  const id = useId();

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let anchor: HTMLElement | null = null;
    const hide = () => { clearTimeout(timer); anchor = null; setActive(null); };
    const find = (target: EventTarget | null) => target instanceof Element ? target.closest<HTMLElement>("[data-tooltip]") : null;
    const needsTooltip = (element: HTMLElement) => {
      const selector = element.dataset.tooltipOverflow;
      if (selector === undefined) return true;
      const label = selector ? element.querySelector<HTMLElement>(selector) : element;
      return Boolean(label && label.clientWidth > 0 && label.scrollWidth > label.clientWidth);
    };
    const show = (next: HTMLElement | null, delay: number) => {
      if (next === anchor) return;
      hide();
      if (!next?.dataset.tooltip) return;
      anchor = next;
      timer = setTimeout(() => {
        // Measure after the hover delay, when row actions have settled into place.
        if (next.isConnected && next.getClientRects().length && needsTooltip(next)) setActive({ anchor: next, text: next.dataset.tooltip!, rows: next.dataset.tooltipRows });
      }, delay);
    };
    const enter = (event: PointerEvent) => { if (event.pointerType !== "touch") show(find(event.target), 500); };
    const leave = (event: PointerEvent) => { if (find(event.relatedTarget) !== anchor) hide(); };
    const focus = (event: FocusEvent) => {
      const next = find(event.target);
      if (document.documentElement.dataset.focusModality !== "pointer" && next?.matches(":focus-visible")) show(next, 0);
    };
    const key = (event: KeyboardEvent) => { if (event.key === "Escape") hide(); };
    document.addEventListener("pointerover", enter, true);
    document.addEventListener("pointerout", leave, true);
    document.addEventListener("pointerdown", hide, true);
    document.addEventListener("focusin", focus);
    document.addEventListener("focusout", hide);
    document.addEventListener("keydown", key, true);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    window.addEventListener("blur", hide);
    const observer = new MutationObserver(() => {
      if (!anchor) return;
      const text = anchor.dataset.tooltip;
      if (!anchor.isConnected || !text) { hide(); return; }
      setActive(current => current && (current.text !== text || current.rows !== anchor?.dataset.tooltipRows) ? { anchor: current.anchor, text, rows: anchor?.dataset.tooltipRows } : current);
    });
    observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-tooltip", "data-tooltip-rows"] });
    return () => {
      clearTimeout(timer); observer.disconnect();
      document.removeEventListener("pointerover", enter, true);
      document.removeEventListener("pointerout", leave, true);
      document.removeEventListener("pointerdown", hide, true);
      document.removeEventListener("focusin", focus);
      document.removeEventListener("focusout", hide);
      document.removeEventListener("keydown", key, true);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
      window.removeEventListener("blur", hide);
    };
  }, []);

  useLayoutEffect(() => {
    const node = tooltip.current;
    if (!active || !node) return;
    const { anchor } = active;
    const descriptions = anchor.getAttribute("aria-describedby");
    anchor.setAttribute("aria-describedby", [descriptions, id].filter(Boolean).join(" "));
    // Use the top layer so scroll containers and other popovers cannot clip the hint.
    if (typeof node.showPopover === "function") {
      node.setAttribute("popover", "manual");
      node.showPopover();
    }
    const rect = anchor.getBoundingClientRect();
    const { width, height } = node.getBoundingClientRect();
    const gutter = 8;
    const left = Math.max(gutter, Math.min(rect.left + (rect.width - width) / 2, window.innerWidth - width - gutter));
    const top = rect.top >= height + gutter * 2 ? rect.top - height - gutter : rect.bottom + gutter;
    node.style.left = `${Math.round(left)}px`;
    node.style.top = `${Math.round(Math.max(gutter, Math.min(top, window.innerHeight - height - gutter)))}px`;
    return () => {
      if (descriptions === null) anchor.removeAttribute("aria-describedby");
      else anchor.setAttribute("aria-describedby", descriptions);
      if (node.hasAttribute("popover") && node.matches(":popover-open")) node.hidePopover();
    };
  }, [active, id]);

  const rows = detailRows(active?.rows);
  return active && createPortal(<div ref={tooltip} id={id} role="tooltip" className={`icon-tooltip${rows.length ? " icon-tooltip-details" : ""}`}>
    {rows.length ? <><strong>{active.text}</strong><dl>{rows.map(([label, value], index) => <div key={index}><dt>{label}</dt><dd>{value}</dd></div>)}</dl></> : active.text}
  </div>, document.body);
}
