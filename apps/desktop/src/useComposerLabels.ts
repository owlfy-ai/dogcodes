import { useLayoutEffect, useRef } from "react";

/** Keep as many labels as fit, in the user's preferred priority order. */
export function useComposerLabels() {
  const ref = useRef<HTMLElement>(null);

  useLayoutEffect(() => {
    const footer = ref.current;
    if (!footer) return;
    const fit = () => {
      const available = footer.clientWidth;
      if (!available) return;
      // Measure natural control widths; the spacer only consumes leftover space.
      for (let stage = 0; stage <= 4; stage++) {
        footer.dataset.labelStage = String(stage);
        const controls = Array.from(footer.children).filter(child =>
          !child.classList.contains("spacer") && getComputedStyle(child).display !== "none",
        );
        const gap = parseFloat(getComputedStyle(footer).columnGap) || 0;
        const width = controls.reduce((sum, child) => sum + child.getBoundingClientRect().width, 0);
        // Include both gaps bordering the spacer.
        if (width + gap * controls.length <= available) break;
      }
    };
    fit();
    const resize = new ResizeObserver(fit);
    resize.observe(footer);
    const mutations = new MutationObserver(fit);
    mutations.observe(footer, { childList: true, characterData: true, subtree: true });
    // UI scale is set on the root, independently of the available column width.
    mutations.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });
    document.fonts?.addEventListener("loadingdone", fit);
    return () => {
      resize.disconnect();
      mutations.disconnect();
      document.fonts?.removeEventListener("loadingdone", fit);
    };
  }, []);

  return ref;
}
