import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { TurnReviewRun } from "@dogcodes/protocol";
import { AgentIcon, providerLabel } from "./agentPresentation";
import { FloatingPopover } from "./FloatingPopover";
import { reviewLabels } from "./ReviewActions";
import { translate as t } from "./i18n";

// Getters so every read follows the current UI language.
export const reviewStates = {
  get queued() { return t("turnActions.state.queued"); },
  get running() { return t("reviewHistory.running"); },
  get cancelling() { return t("turnActions.state.cancelling"); },
  get completed() { return t("turnActions.state.completed"); },
  get failed() { return t("turnActions.state.failed"); },
  get cancelled() { return t("turnActions.state.cancelled"); },
  get interrupted() { return t("turnActions.state.interrupted"); },
};
export const reviewActive = (run: TurnReviewRun) => ["queued", "running", "cancelling"].includes(run.status);
const label = (run: TurnReviewRun) => reviewLabels[run.request.kind as keyof typeof reviewLabels] ?? run.request.kind;
const description = (run: TurnReviewRun) => `${label(run)} · ${providerLabel(run.request.executor.providerId)} · ${reviewStates[run.status]}`;
export const newestReviews = (runs: TurnReviewRun[]) => [...runs].reverse().sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));

export function ReviewHistoryTitle({ run }: { run: TurnReviewRun }) {
  return <span className="project-review-history-title"><b>{label(run)}</b><span className="project-review-history-status" data-status={run.status}>{reviewStates[run.status]}</span>{run.status === "completed" && run.report?.structured && <span className="project-review-history-findings" data-has-findings={run.report.findings.length > 0}>{t("reviewResult.findingCount", { count: run.report.findings.length })}</span>}</span>;
}

export function ProjectReviewHistory({ runs, selected, onSelect }: { runs: TurnReviewRun[]; selected: TurnReviewRun; onSelect: (id: string) => void }) {
  const [open, setOpen] = useState(false);
  const host = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const otherActive = runs.filter(run => run.request.runId !== selected.request.runId && reviewActive(run)).length;
  useEffect(() => {
    if (!open) return;
    host.current?.querySelector<HTMLElement>('[role="menuitemradio"][aria-checked="true"]')?.focus();
    const outside = (event: PointerEvent) => { if (!host.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { setOpen(false); trigger.current?.focus(); } };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open]);
  return <div className="project-review-history" ref={host}>
    <button ref={trigger} type="button" className="project-review-history-trigger" aria-label={t("reviewHistory.select")} data-tooltip={description(selected)} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(value => !value)}>
      <AgentIcon providerId={selected.request.executor.providerId}/><ReviewHistoryTitle run={selected}/>
      <time className="project-review-history-time" dateTime={selected.createdAt}>{new Date(selected.createdAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time>
      {runs.length > 1 && <small className="project-review-history-count">{t("reviewHistory.count", { count: runs.length })}</small>}
      <svg className="project-review-history-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m4 6 4 4 4-4"/></svg>
    </button>
    {otherActive > 0 && <small className="project-review-other-active" role="status">{t("reviewHistory.otherActive", { count: otherActive })}</small>}
    {open && <FloatingPopover className="agent-picker-menu project-review-history-menu" role="menu" aria-label={t("reviewHistory.title")} onKeyDown={event => {
      const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "ArrowDown" ? (index + 1) % buttons.length : event.key === "ArrowUp" ? (index - 1 + buttons.length) % buttons.length : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : null;
      if (next !== null) { event.preventDefault(); buttons[next]?.focus(); }
      if (event.key === "Tab") setOpen(false);
    }}>
      <header>{t("reviewHistory.title")} · {runs.length}</header>
      {runs.map(run => {
        const chosen = run.request.runId === selected.request.runId;
        const createdAt = new Date(run.createdAt).toLocaleString();
        return <button type="button" role="menuitemradio" aria-checked={chosen} data-tooltip={description(run)} className={chosen ? "active" : ""} key={run.request.runId} onClick={() => { onSelect(run.request.runId); setOpen(false); trigger.current?.focus(); }}>
          <AgentIcon providerId={run.request.executor.providerId}/><span className="agent-option-label">
            <ReviewHistoryTitle run={run}/>
            <small className="project-review-history-meta"><time dateTime={run.createdAt} data-tooltip={createdAt}>{createdAt}</time></small>
          </span><i aria-hidden="true">{chosen ? "✓" : ""}</i>
        </button>;
      })}
    </FloatingPopover>}
  </div>;
}

// Keep each report mounted so its findings, tool details and repair selection survive switching.
export function ProjectReviewPage({ selected, runId, children }: { selected: boolean; runId: string; children: ReactNode }) {
  const element = useRef<HTMLDivElement>(null);
  const scroll = useRef(0);
  useLayoutEffect(() => {
    const node = element.current;
    if (!node || !selected) return;
    const measure = () => node.style.setProperty("--review-scrollbar-width", `${Math.max(0, node.offsetWidth - node.clientWidth)}px`);
    measure();
    node.scrollTop = scroll.current;
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    observer?.observe(node);
    if (node.firstElementChild) observer?.observe(node.firstElementChild);
    return () => observer?.disconnect();
  }, [selected]);
  return <div ref={element} className="project-review-results" hidden={!selected} data-review-id={runId} onScroll={event => { if (selected) scroll.current = event.currentTarget.scrollTop; }}><div className="project-review-result">{children}</div></div>;
}
