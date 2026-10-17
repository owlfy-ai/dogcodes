import { useEffect, useState } from "react";

export const loadingShowDelayMs = 300;
export const loadingMinimumMs = 400;

/**
 * Timing for a loading surface: nothing for the first 300ms, so fast reads never
 * flash, and once shown it stays at least 400ms. `pending` is whether the
 * surface should still stand in for content; `visible` is whether its indicator shows.
 */
export function useLoadingVisibility(active: boolean, delay = loadingShowDelayMs, minimum = loadingMinimumMs) {
  const [shownAt, setShownAt] = useState<number | null>(null);
  // Derived during render, so content is never painted for a frame before the hold starts.
  const lingering = !active && shownAt !== null && Date.now() < shownAt + minimum;
  useEffect(() => {
    if (active) {
      if (shownAt !== null) return;
      const timer = setTimeout(() => setShownAt(Date.now()), delay);
      return () => clearTimeout(timer);
    }
    if (shownAt === null) return;
    const timer = setTimeout(() => setShownAt(null), Math.max(0, shownAt + minimum - Date.now()));
    return () => clearTimeout(timer);
  }, [active, shownAt, delay, minimum]);
  return { pending: active || lingering, visible: shownAt !== null && (active || lingering), lingering };
}
