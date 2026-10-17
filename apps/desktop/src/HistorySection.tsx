import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { FloatingPopover } from "./FloatingPopover";
import type { HistorySort, SectionPreference } from "./historySections";
import "./historySections.css";

export function HistorySection({ label, className, preference, onToggle, onSort, onCreate, sortDisabled, children }: {
  label: string; className: string; preference: SectionPreference; onToggle: () => void;
  sortDisabled?: boolean;
  onSort: (sort: HistorySort) => boolean; onCreate?: () => void; children: ReactNode;
}) {
  const [menuOpen, setMenuOpen] = useState(false);
  const controls = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const bodyId = useId(), menuId = useId();
  useEffect(() => {
    if (!menuOpen) return;
    controls.current?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus();
    const outside = (event: PointerEvent) => { if (!controls.current?.contains(event.target as Node)) setMenuOpen(false); };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [menuOpen]);
  return <section className={`history-section ${className}`} aria-label={label}>
    <header className="history-section-header">
      <h2 className="history-section-label"><button type="button" className="history-section-toggle" aria-expanded={!preference.collapsed} aria-controls={bodyId} onClick={onToggle}>
        {label}<svg className="history-section-chevron" viewBox="0 0 16 16" aria-hidden="true" style={{ transform: preference.collapsed ? "rotate(-90deg)" : undefined }}><path d="m4 6 4 4 4-4" /></svg>
      </button></h2>
      <div ref={controls} className="history-section-controls" onBlur={event => {
        // WKWebView does not focus buttons on pointer clicks: a null target
        // must not unmount the menu before click. Outside pointers close above.
        if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setMenuOpen(false);
      }}>
        <button ref={trigger} type="button" className="history-section-action" disabled={sortDisabled} aria-label={`${label}排序方式`} data-tooltip="排序方式" aria-haspopup="menu" aria-expanded={menuOpen} aria-controls={menuOpen ? menuId : undefined} onClick={() => setMenuOpen(open => !open)}>
          <svg className="history-sort-icon" viewBox="0 0 20 20" aria-hidden="true"><path d="M4 4v12m-3-3 3 3 3-3M10 5h7M10 10h5M10 15h3" /></svg>
        </button>
        {onCreate && <button type="button" className="history-section-action" aria-label="新建项目" data-tooltip="新建项目" onClick={onCreate}>＋</button>}
        {menuOpen && <FloatingPopover id={menuId} placement="bottom" className="history-sort-menu" role="menu" aria-label={`${label}排序方式`} onKeyDown={event => {
          const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')];
          const index = items.indexOf(document.activeElement as HTMLButtonElement);
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setMenuOpen(false); trigger.current?.focus(); }
          if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
            event.preventDefault();
            items[event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
          }
        }}>
          {([["updated", "最近更新"], ["manual", "手动排序"]] as const).map(([value, text]) => <button key={value} type="button" role="menuitemradio" disabled={sortDisabled} aria-checked={preference.sort === value} onClick={() => {
            if (onSort(value)) { setMenuOpen(false); trigger.current?.focus(); }
          }}><span>{text}</span><span aria-hidden="true">{preference.sort === value ? "✓" : ""}</span></button>)}
        </FloatingPopover>}
      </div>
    </header>
    <div id={bodyId} className="history-section-body" hidden={preference.collapsed}>{children}</div>
  </section>;
}
