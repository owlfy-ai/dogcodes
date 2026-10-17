import { useEffect, useState } from "react";
import { PanelResizeHandle } from "./PanelResizeHandle";

const WIDTH_KEY = "dogcodes.review.width";
export const DEFAULT_REVIEW_WIDTH = 380;
export const MIN_REVIEW_WIDTH = 260;
export function reviewWidthLimit(viewportWidth: number, sidebarWidth = 220) {
  return Math.max(MIN_REVIEW_WIDTH, Math.min(900, viewportWidth - sidebarWidth - 320));
}
export function useReviewWidth(sidebarWidth = 220) {
  const [preferred, setPreferred] = useState(() => {
    try {
      const value = Number(localStorage.getItem(WIDTH_KEY));
      return Number.isFinite(value) && value >= MIN_REVIEW_WIDTH ? Math.min(value, 900) : DEFAULT_REVIEW_WIDTH;
    } catch { return DEFAULT_REVIEW_WIDTH; }
  });
  const [viewport, setViewport] = useState(window.innerWidth);
  useEffect(() => {
    const resize = () => setViewport(window.innerWidth);
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);
  const max = reviewWidthLimit(viewport, sidebarWidth);
  const width = Math.min(preferred, max);
  const setWidth = (value: number) => {
    const next = Math.round(Math.max(MIN_REVIEW_WIDTH, Math.min(value, max)));
    setPreferred(next);
    try { localStorage.setItem(WIDTH_KEY, String(next)); } catch { /* Resizing still works when storage is unavailable. */ }
  };
  return { width, max, setWidth };
}
export function ReviewResizeHandle(props: { width: number; max: number; onResize: (width: number) => void }) {
  return <PanelResizeHandle {...props} side="right" min={MIN_REVIEW_WIDTH} defaultWidth={DEFAULT_REVIEW_WIDTH} />;
}
