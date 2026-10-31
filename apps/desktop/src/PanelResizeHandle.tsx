import { useRef, useState } from "react";
import { translate as t } from "./i18n";

export function PanelResizeHandle({ width, min, max, defaultWidth, side, onResize, className, label }: { width: number; min: number; max: number; defaultWidth: number; side: "left" | "right"; onResize: (width: number) => void; className?: string; label?: string }) {
  const drag = useRef<{ pointerId: number; x: number; width: number } | null>(null);
  const [dragging, setDragging] = useState(false);
  return <div role="separator" aria-label={label ?? t(side === "left" ? "resize.left" : "resize.right")} aria-orientation="vertical" aria-valuemin={min} aria-valuemax={max} aria-valuenow={width}
    tabIndex={0} className={`${className ?? `review-resize-handle${side === "left" ? " sidebar-resize-handle" : ""}`}${dragging ? " is-dragging" : ""}`} data-tooltip={t("resize.widthTooltip")}
    onPointerDown={event => {
      if (event.button !== 0) return;
      event.preventDefault();
      event.currentTarget.setPointerCapture(event.pointerId);
      drag.current = { pointerId: event.pointerId, x: event.clientX, width };
      setDragging(true);
    }}
    onPointerMove={event => {
      const start = drag.current;
      if (start && start.pointerId === event.pointerId) onResize(start.width + (event.clientX - start.x) * (side === "left" ? 1 : -1));
    }}
    onPointerUp={event => {
      if (drag.current?.pointerId !== event.pointerId) return;
      drag.current = null; setDragging(false);
      event.currentTarget.releasePointerCapture(event.pointerId);
    }}
    onLostPointerCapture={() => { drag.current = null; setDragging(false); }}
    onPointerCancel={() => { drag.current = null; setDragging(false); }}
    onDoubleClick={() => onResize(defaultWidth)}
    onKeyDown={event => {
      const value = event.key === "ArrowLeft" ? width + (side === "left" ? -20 : 20) : event.key === "ArrowRight" ? width + (side === "left" ? 20 : -20) : event.key === "Home" ? min : event.key === "End" ? max : null;
      if (value !== null) { event.preventDefault(); onResize(value); }
    }} />;
}
