import { useLayoutEffect, useRef, type ComponentProps } from "react";

/** Keep DOM ancestry for theme inheritance and outside-click handling, but paint in the top layer. */
export function FloatingPopover({ open = true, point, placement, align = "end", layout = "block", ...props }: ComponentProps<"div"> & { open?: boolean; point?: { x: number; y: number }; placement?: "right" | "bottom"; align?: "start" | "end"; layout?: "block" | "column" }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const node = ref.current;
    const parent = node?.parentElement;
    if (!node || !parent || typeof node.showPopover !== "function") return;
    const details = parent.closest("details");
    node.setAttribute("popover", "manual");
    const position = () => {
      const visible = open && (!details || details.open);
      if (!visible) { if (node.matches(":popover-open")) node.hidePopover(); node.style.setProperty("display", "none", "important"); return; }
      if (!node.matches(":popover-open")) node.showPopover();
      const anchor = parent.querySelector<HTMLElement>('[aria-expanded="true"], summary') ?? parent;
      const rect = anchor.getBoundingClientRect();
      const gutter = 8;
      // Fractional anchors must not put the menu's artwork between device pixels.
      const pixelRatio = window.devicePixelRatio || 1;
      const snap = (value: number) => Math.round(value * pixelRatio) / pixelRatio;
      Object.assign(node.style, { position: "fixed", margin: "0", bottom: "auto", right: "auto", minWidth: "0", boxSizing: "border-box", maxWidth: `calc(100vw - ${gutter * 2}px)`, maxHeight: `calc(100vh - ${gutter * 2}px)`, overflow: layout === "column" ? "hidden" : "auto", flexDirection: "column", whiteSpace: "normal" });
      node.style.setProperty("display", layout === "column" ? "flex" : "block", "important");
      const width = node.getBoundingClientRect().width;
      const height = node.getBoundingClientRect().height;
      if (placement === "right" && !point) {
        const left = rect.right + gutter + width <= window.innerWidth ? rect.right + gutter : rect.left - width - gutter;
        node.style.left = `${snap(Math.max(gutter, Math.min(left, window.innerWidth - width - gutter)))}px`;
        node.style.top = `${snap(Math.max(gutter, Math.min(rect.top, window.innerHeight - height - gutter)))}px`;
        return;
      }
      const above = Math.max(0, rect.top - gutter * 2);
      const below = Math.max(0, window.innerHeight - rect.bottom - gutter * 2);
      const upward = placement === "bottom"
        ? height > below && above > below
        : height <= above || (height > below && above > below);
      const available = upward ? above : below;
      // If neither side fits, scroll on the roomier side instead of covering the trigger.
      if (!point) node.style.maxHeight = `${available}px`;
      const visibleHeight = Math.min(height, available);
      const top = upward ? rect.top - visibleHeight - gutter : rect.bottom + gutter;
      node.style.left = `${snap(Math.max(gutter, Math.min(point?.x ?? (align === "start" ? rect.left : rect.right - width), window.innerWidth - width - gutter)))}px`;
      node.style.top = `${snap(Math.max(gutter, point ? Math.min(point.y, window.innerHeight - height - gutter) : top))}px`;
    };
    const update = () => {
      // Measuring the unconstrained menu can temporarily remove its scrollbar.
      // Restore the reading position after applying the final viewport bounds.
      const scroller = layout === "column" ? node.querySelector<HTMLElement>("[data-popover-scroll]") ?? node : node;
      const { scrollTop, scrollLeft } = scroller;
      position();
      scroller.scrollTop = scrollTop;
      scroller.scrollLeft = scrollLeft;
    };
    const onScroll = (event: Event) => {
      // Internal scrolling does not move the anchor. Repositioning here would
      // repeatedly clamp the user's wheel/trackpad movement during measurement.
      if (event.target instanceof Node && node.contains(event.target)) return;
      update();
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(parent); observer.observe(node);
    window.addEventListener("resize", update);
    window.addEventListener("scroll", onScroll, true);
    details?.addEventListener("toggle", update);
    return () => {
      observer.disconnect(); window.removeEventListener("resize", update); window.removeEventListener("scroll", onScroll, true); details?.removeEventListener("toggle", update);
      if (node.matches(":popover-open")) node.hidePopover();
      node.style.removeProperty("display");
    };
  }, [open, point?.x, point?.y, placement, align, layout]);
  return <div {...props} ref={ref} />;
}
