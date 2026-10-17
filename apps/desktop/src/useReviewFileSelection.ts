import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent } from "react";
import type { ReviewSnapshot } from "./reviewClient";

export const reviewFileKey = (rootId: string, path: string) => JSON.stringify([rootId, path]);
const rows = (nav: HTMLElement) => [...nav.querySelectorAll<HTMLButtonElement>("button[data-review-key]")];
const rowAt = (target: EventTarget | null) => target instanceof Element ? target.closest<HTMLButtonElement>("button[data-review-key]") : null;
type Drag = { id: number; startX: number; startY: number; x: number; y: number; clientX: number; clientY: number; original: Set<string>; originalAnchor: string | null; additive: boolean; active: boolean };

/** Selection is independent of the open preview. Ranges follow visible tree order. */
export function useReviewFileSelection(snapshot: ReviewSnapshot | null, scope: string, disabled: boolean) {
  const nav = useRef<HTMLElement>(null);
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const anchor = useRef<string | null>(null);
  const drag = useRef<Drag | null>(null);
  const suppressClick = useRef(false);
  const [rectangle, setRectangle] = useState<{ left: number; top: number; width: number; height: number } | null>(null);
  useEffect(() => { setSelected(new Set()); anchor.current = null; }, [scope]);
  useEffect(() => {
    const valid = new Set(snapshot?.roots.flatMap(root => root.files.map(file => reviewFileKey(root.root.id, file.path))));
    setSelected(previous => new Set([...previous].filter(key => valid.has(key))));
    if (anchor.current && !valid.has(anchor.current)) anchor.current = null;
  }, [snapshot]);
  useEffect(() => {
    let frame = 0;
    let timer = 0;
    const release = (cancel: boolean) => {
      const current = drag.current;
      if (!current) return;
      drag.current = null;
      cancelAnimationFrame(frame);
      if (current.active) {
        if (cancel) { setSelected(current.original); anchor.current = current.originalAnchor; }
        suppressClick.current = true;
        timer = window.setTimeout(() => { suppressClick.current = false; }, 0);
      }
      setRectangle(null);
      if (nav.current?.hasPointerCapture?.(current.id)) nav.current.releasePointerCapture(current.id);
    };
    const paint = () => {
      const current = drag.current, element = nav.current;
      if (!current?.active || !element) return;
      const bounds = element.getBoundingClientRect();
      current.x = Math.max(0, Math.min(element.clientWidth, current.clientX - bounds.left)) + element.scrollLeft;
      current.y = Math.max(0, Math.min(element.clientHeight, current.clientY - bounds.top)) + element.scrollTop;
      const box = { left: Math.min(current.startX, current.x), top: Math.min(current.startY, current.y), width: Math.abs(current.x - current.startX), height: Math.abs(current.y - current.startY) };
      setRectangle(box);
      const hits = rows(element).filter(row => {
        const rect = row.getBoundingClientRect();
        const left = rect.left - bounds.left + element.scrollLeft, top = rect.top - bounds.top + element.scrollTop;
        return left < box.left + box.width && left + rect.width > box.left && top < box.top + box.height && top + rect.height > box.top;
      }).map(row => row.dataset.reviewKey!);
      setSelected(new Set([...(current.additive ? current.original : []), ...hits]));
      if (hits.length) anchor.current = hits[0];
    };
    const scroll = () => {
      const current = drag.current, element = nav.current;
      if (!current?.active || !element) return;
      const bounds = element.getBoundingClientRect();
      const speed = current.clientY < bounds.top + 28 ? -10 : current.clientY > bounds.bottom - 28 ? 10 : 0;
      if (speed) { const previous = element.scrollTop; element.scrollTop += speed; if (element.scrollTop !== previous) paint(); }
      frame = requestAnimationFrame(scroll);
    };
    const move = (event: PointerEvent) => {
      const current = drag.current;
      if (!current || event.pointerId !== current.id) return;
      current.clientX = event.clientX; current.clientY = event.clientY;
      if (!current.active) {
        if (Math.hypot(event.clientX - current.x, event.clientY - current.y) < 5) return;
        current.active = true;
        nav.current?.focus({ preventScroll: true });
        nav.current?.setPointerCapture?.(event.pointerId);
        frame = requestAnimationFrame(scroll);
      }
      event.preventDefault(); paint();
    };
    const up = (event: PointerEvent) => { if (drag.current?.id === event.pointerId) release(event.type !== "pointerup"); };
    const cancel = () => release(true);
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape" && drag.current?.active) { event.preventDefault(); event.stopPropagation(); cancel(); } };
    window.addEventListener("pointermove", move, { passive: false });
    window.addEventListener("pointerup", up); window.addEventListener("pointercancel", up);
    window.addEventListener("blur", cancel); window.addEventListener("keydown", escape, true);
    return () => {
      release(true); window.clearTimeout(timer); suppressClick.current = false;
      window.removeEventListener("pointermove", move); window.removeEventListener("pointerup", up); window.removeEventListener("pointercancel", up);
      window.removeEventListener("blur", cancel); window.removeEventListener("keydown", escape, true);
    };
  }, [disabled, scope, snapshot]);
  const clear = () => { setSelected(new Set()); anchor.current = null; };
  return {
    nav, selected, rectangle, clear,
    remove: (keys: string[]) => setSelected(previous => new Set([...previous].filter(key => !keys.includes(key)))),
    contextSelect: (rootId: string, path: string) => {
      const key = reviewFileKey(rootId, path);
      if (!selected.has(key)) { setSelected(new Set([key])); anchor.current = key; }
    },
    onPointerDown: (event: ReactPointerEvent<HTMLElement>) => {
      if (disabled || event.button !== 0 || event.pointerType === "touch") return;
      if (!event.currentTarget.querySelector("button[data-review-key]:not(:disabled)")) return;
      const target = event.target as Element;
      if (target.closest("button, input, summary, a") && !rowAt(target)) return;
      const element = event.currentTarget, bounds = element.getBoundingClientRect();
      // Leave native scrollbar gestures alone.
      if (event.clientX >= bounds.left + element.clientWidth || event.clientY >= bounds.top + element.clientHeight) return;
      drag.current = { id: event.pointerId, startX: event.clientX - bounds.left + element.scrollLeft, startY: event.clientY - bounds.top + element.scrollTop, x: event.clientX, y: event.clientY, clientX: event.clientX, clientY: event.clientY, original: new Set(selected), originalAnchor: anchor.current, additive: event.ctrlKey || event.metaKey || event.shiftKey, active: false };
    },
    onClickCapture: (event: ReactMouseEvent<HTMLElement>) => {
      if (suppressClick.current) { event.preventDefault(); event.stopPropagation(); return; }
      if (disabled) return;
      const row = rowAt(event.target);
      if (!row) { if (!(event.target as Element).closest("button, summary, a")) clear(); return; }
      const key = row.dataset.reviewKey!;
      if (event.shiftKey || event.ctrlKey || event.metaKey) {
        event.preventDefault(); event.stopPropagation(); row.focus();
        const ordered = rows(event.currentTarget).map(row => row.dataset.reviewKey!);
        const start = anchor.current ? ordered.indexOf(anchor.current) : -1, end = ordered.indexOf(key);
        if (event.shiftKey && start >= 0) {
          const range = ordered.slice(Math.min(start, end), Math.max(start, end) + 1);
          setSelected(new Set([...(event.ctrlKey || event.metaKey ? selected : []), ...range]));
        } else {
          setSelected(previous => { const next = new Set(event.shiftKey ? [] : previous); if (next.has(key)) next.delete(key); else next.add(key); return next; });
          anchor.current = key;
        }
      } else { setSelected(new Set([key])); anchor.current = key; }
    },
    onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => {
      if (disabled) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
        event.preventDefault(); setSelected(new Set(rows(event.currentTarget).map(row => row.dataset.reviewKey!)));
      } else if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); clear(); }
    },
  };
}
