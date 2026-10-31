import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { translate as t } from "./i18n";

const HEIGHT_KEY = "dogcodes.review.heightRatio";
const DEFAULT_RATIO = .45;
const HANDLE_HEIGHT = 7;

/** Files and diff fill the panel; the review dock sits below them and can be resized when expanded. */
export function ReviewDiffSplit({ review, expanded, children }: { review: ReactNode; expanded: boolean; children: ReactNode }) {
  const container = useRef<HTMLDivElement>(null);
  const drag = useRef<{ id: number; y: number; height: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  const [available, setAvailable] = useState(0);
  const [ratio, setRatio] = useState(() => {
    try {
      const value = Number(localStorage.getItem(HEIGHT_KEY));
      return value > 0 && value < 1 ? value : DEFAULT_RATIO;
    } catch { return DEFAULT_RATIO; }
  });
  useLayoutEffect(() => {
    const element = container.current!;
    const measure = () => setAvailable(Math.max(0, element.getBoundingClientRect().height - HANDLE_HEIGHT));
    measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(element);
    window.addEventListener("resize", measure);
    return () => { observer?.disconnect(); window.removeEventListener("resize", measure); };
  }, []);
  const min = Math.min(180, available * .6);
  const max = Math.max(min, available - Math.min(140, available * .4));
  const height = Math.round(Math.max(min, Math.min(max, available * ratio)));
  const resize = (value: number) => {
    if (!available) return;
    const next = Math.max(min, Math.min(max, value)) / available;
    setRatio(next);
    try { localStorage.setItem(HEIGHT_KEY, String(next)); } catch { /* Resizing works without storage. */ }
  };
  const stop = () => { drag.current = null; setDragging(false); };
  return <div ref={container} className={`review-diff-split${expanded ? " is-expanded" : ""}${dragging ? " is-dragging" : ""}`}>
    <div className="review-diff-files">{children}</div>
    {expanded && <div role="separator" tabIndex={0} aria-label={t("resize.reviewHeight")} aria-orientation="horizontal"
      aria-valuemin={Math.round(min)} aria-valuemax={Math.round(max)} aria-valuenow={height}
      className="review-height-handle" data-tooltip={t("resize.reviewHeightTooltip")}
      onPointerDown={event => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        drag.current = { id: event.pointerId, y: event.clientY, height };
        setDragging(true);
      }}
      onPointerMove={event => {
        const start = drag.current;
        if (start?.id === event.pointerId) resize(start.height - (event.clientY - start.y));
      }}
      onPointerUp={event => {
        if (drag.current?.id !== event.pointerId) return;
        stop(); event.currentTarget.releasePointerCapture(event.pointerId);
      }}
      onPointerCancel={stop} onLostPointerCapture={stop}
      onDoubleClick={() => resize(available * DEFAULT_RATIO)}
      onKeyDown={event => {
        const next = event.key === "ArrowUp" ? height + 20 : event.key === "ArrowDown" ? height - 20 : event.key === "Home" ? max : event.key === "End" ? min : null;
        if (next !== null) { event.preventDefault(); resize(next); }
      }} />}
    <div className="review-diff-review" style={expanded ? { height: available ? height : "45%" } : undefined}>{review}</div>
  </div>;
}
