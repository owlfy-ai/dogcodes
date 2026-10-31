import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { PanelResizeHandle } from "./PanelResizeHandle";
import { translate as t } from "./i18n";

const WIDTH_KEY = "dogcodes.review.filesWidthRatio";
const DEFAULT_RATIO = .5;
const HANDLE_WIDTH = 7;

/** Only the file list and reader share this split; Git actions stay above it. */
export function ReviewFilesSplit({ files, children, active, busy }: { files: ReactNode; children: ReactNode; active: boolean; busy: boolean }) {
  const container = useRef<HTMLDivElement>(null);
  const [available, setAvailable] = useState(0);
  const [ratio, setRatio] = useState(() => {
    try {
      const value = Number(localStorage.getItem(WIDTH_KEY));
      return value > 0 && value < 1 ? value : DEFAULT_RATIO;
    } catch { return DEFAULT_RATIO; }
  });
  useLayoutEffect(() => {
    const element = container.current!;
    const measure = () => setAvailable(Math.max(0, element.getBoundingClientRect().width - HANDLE_WIDTH));
    measure();
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(element);
    window.addEventListener("resize", measure);
    return () => { observer?.disconnect(); window.removeEventListener("resize", measure); };
  }, []);
  const min = Math.round(Math.min(160, available * .4));
  const max = Math.max(min, available - min);
  const width = Math.round(Math.max(min, Math.min(max, available * ratio)));
  const resize = (value: number) => {
    if (!available) return;
    const next = Math.max(min, Math.min(max, value)) / available;
    setRatio(next);
    try { localStorage.setItem(WIDTH_KEY, String(next)); } catch { /* Resizing works without storage. */ }
  };
  return <div ref={container} className={`review-body${active ? "" : " review-list-only"}`} aria-busy={busy}
    style={{ gridTemplateColumns: active ? `${available ? `${width}px` : "minmax(0, 1fr)"} ${HANDLE_WIDTH}px minmax(0, 1fr)` : "minmax(0, 1fr)" }}>
    {files}
    {active && <PanelResizeHandle width={width} min={min} max={max} defaultWidth={available * DEFAULT_RATIO} side="left"
      onResize={resize} className="review-files-resize-handle" label={t("resize.filesWidth")} />}
    {children}
  </div>;
}
