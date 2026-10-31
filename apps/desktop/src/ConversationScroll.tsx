import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { translate as t } from "./i18n";
export function ConversationScroll({
  children,
  taskId,
  latestUserMessageId,
  active = true,
}: {
  children: ReactNode;
  taskId: string;
  latestUserMessageId?: string;
  active?: boolean;
}) {
  const viewport = useRef<HTMLDivElement>(null),
    content = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  const lastTop = useRef(0);
  const summaryPositions = useRef(new WeakMap<HTMLDetailsElement, number>());
  const summaryToggle = useRef<{ details: HTMLDetailsElement; open: boolean; top?: number } | null>(null);
  const restoredTop = useRef<number | null>(null);
  const visible = useRef(active);
  visible.current = active;
  const [away, setAway] = useState(false);
  const bottom = () => {
    const node = viewport.current;
    if (node && visible.current) {
      restoredTop.current = null;
      node.scrollTop = node.scrollHeight;
      lastTop.current = node.scrollTop;
      follow.current = true;
      setAway(false);
    }
  };
  const trackScroll = (node: HTMLDivElement) => {
    const distance = node.scrollHeight - node.scrollTop - node.clientHeight;
    // Native scrollbar drags may arrive without a pointer event on the viewport.
    // Pause on upward movement, and resume only after moving back to the bottom.
    if (node.scrollTop < lastTop.current && distance > 1) follow.current = false;
    else if (node.scrollTop > lastTop.current && distance <= 1) follow.current = true;
    lastTop.current = node.scrollTop;
    setAway(!follow.current && distance > 1);
  };
  useLayoutEffect(() => {
    summaryPositions.current = new WeakMap();
    summaryToggle.current = null;
    follow.current = true;
    setAway(false);
    bottom();
  // A new prompt explicitly resumes following, even after reading older messages.
  // Use its identity rather than all timeline changes so streamed replies still
  // respect subsequent manual scrolling.
  }, [taskId, latestUserMessageId]);
  useLayoutEffect(() => {
    if (!active || !viewport.current) return;
    if (follow.current) bottom();
    else viewport.current.scrollTop = lastTop.current;
  }, [active]);
  useLayoutEffect(() => {
    const node = content.current;
    if (!node) return;
    const observer = new ResizeObserver(() => {
      if (summaryToggle.current?.open === false) return;
      if (visible.current && follow.current && !window.getSelection()?.toString()) {
        // ResizeObserver can run before the pending native scroll event.
        if (viewport.current) trackScroll(viewport.current);
        if (follow.current) bottom();
      }
    });
    observer.observe(node);
    const scrollport = viewport.current;
    const onToggle = (event: Event) => {
      const request = summaryToggle.current;
      if (!request || !scrollport || event.target !== request.details || request.details.open !== request.open) return;
      summaryToggle.current = null;
      // Let native details activation and the card's expansion cache stay in
      // sync, then measure the final layout before scrolling.
      if (request.open) bottom();
      else if (request.top !== undefined) {
        follow.current = false;
        scrollport.scrollTop = request.top;
        lastTop.current = scrollport.scrollTop;
        restoredTop.current = scrollport.scrollTop;
        const distance = scrollport.scrollHeight - scrollport.scrollTop - scrollport.clientHeight;
        follow.current = distance <= 1;
        setAway(distance > 1);
        summaryPositions.current.delete(request.details);
      }
    };
    if (scrollport) {
      observer.observe(scrollport);
      scrollport.addEventListener("toggle", onToggle, true);
    }
    return () => { observer.disconnect(); scrollport?.removeEventListener("toggle", onToggle, true); };
  }, []);
  return (
    <div
      className="timeline"
      ref={viewport}
      aria-label={t("conversationScroll.label")}
      style={{ overflowAnchor: away ? "auto" : "none", scrollBehavior: "auto" }}
      onWheel={event => { if (event.deltaY < 0) follow.current = false; }}
      onTouchStart={() => { follow.current = false; }}
      onPointerDown={() => { follow.current = false; }}
      onClickCapture={event => {
        const node = viewport.current;
        const target = event.target instanceof Element ? event.target : null;
        const heading = target?.closest("summary.turn-summary-heading");
        const details = heading?.parentElement;
        if (!node || !(details instanceof HTMLDetailsElement) || event.defaultPrevented
          || target?.closest("button, a, input, select, textarea, [role='button']")) return;
        const cards = content.current?.querySelectorAll<HTMLDetailsElement>("details.turn-summary");
        const previousTop = summaryPositions.current.get(details);
        if (details.open && previousTop !== undefined) {
          follow.current = false;
          summaryToggle.current = { details, open: false, top: previousTop };
        } else if (!details.open && cards?.[cards.length - 1] === details) {
          summaryPositions.current.set(details, node.scrollTop);
          summaryToggle.current = { details, open: true };
          follow.current = true;
        }
      }}
      onKeyDown={event => { if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(event.key)) follow.current = false; }}
      onScroll={() => {
        const node = viewport.current;
        if (!node || !visible.current) return;
        if (summaryToggle.current?.open === false) return;
        if (restoredTop.current !== null) {
          const restored = restoredTop.current;
          restoredTop.current = null;
          if (Math.abs(node.scrollTop - restored) <= 1) { lastTop.current = node.scrollTop; return; }
        }
        trackScroll(node);
        if (follow.current) bottom();
      }}
    >
      <div className="conversation-flow" ref={content}>{children}</div>
      {away && (
        <button className="conversation-latest" type="button" onClick={bottom}>
          ↓ {t("conversationScroll.toBottom")}
        </button>
      )}
    </div>
  );
}
