import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
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
      if (follow.current && !window.getSelection()?.toString()) bottom();
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
        setAway(distance > 80);
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
      aria-label="对话正文"
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
        lastTop.current = node.scrollTop;
        if (restoredTop.current !== null) {
          const restored = restoredTop.current;
          restoredTop.current = null;
          if (Math.abs(node.scrollTop - restored) <= 1) return;
        }
        // Layout and programmatic scroll events must not cancel bottom-follow.
        if (follow.current) { bottom(); return; }
        const isAway =
          node.scrollHeight - node.scrollTop - node.clientHeight > 80;
        follow.current = !isAway;
        setAway(isAway);
      }}
    >
      <div className="conversation-flow" ref={content}>{children}</div>
      {away && (
        <button className="conversation-latest" type="button" onClick={bottom}>
          ↓ 回到底部
        </button>
      )}
    </div>
  );
}
