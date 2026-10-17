import { useEffect, useState } from "react";
import { PanelResizeHandle } from "./PanelResizeHandle";
import { MIN_REVIEW_WIDTH } from "./ReviewResizeHandle";

const WIDTH_KEY = "dogcodes.sidebar.width";
const MIN_WIDTH = 200;
export function useSidebarWidth() {
  const [viewport, setViewport] = useState(window.innerWidth);
  const defaultWidth = viewport <= 1180 ? 240 : 286;
  const [preferred, setPreferred] = useState<number | null>(() => {
    try {
      const value = Number(localStorage.getItem(WIDTH_KEY));
      return Number.isFinite(value) && value >= MIN_WIDTH ? Math.min(value, 480) : null;
    } catch { return null; }
  });
  useEffect(() => {
    const resize = () => setViewport(window.innerWidth);
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);
  // Reserve room for both readers independently of whether the right dock is open.
  const max = Math.max(MIN_WIDTH, Math.min(480, viewport - 320 - MIN_REVIEW_WIDTH));
  const width = Math.min(preferred ?? defaultWidth, max);
  const setWidth = (value: number) => {
    const next = Math.round(Math.max(MIN_WIDTH, Math.min(value, max)));
    setPreferred(next);
    try { localStorage.setItem(WIDTH_KEY, String(next)); } catch { /* Storage is optional. */ }
  };
  return { width, max, defaultWidth, setWidth };
}
export function SidebarResizeHandle({ width, max, defaultWidth, onResize }: { width: number; max: number; defaultWidth: number; onResize: (width: number) => void }) {
  return <PanelResizeHandle side="left" min={MIN_WIDTH} {...{ width, max, defaultWidth, onResize }} />;
}
