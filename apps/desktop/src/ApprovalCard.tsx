import { useEffect, useRef, useState } from "react";
import type { PendingApproval } from "@dogcodes/protocol";
import { QuestionCard } from "./QuestionCard";
import { ApprovalDetails } from "./ApprovalDetails";
import { approvalPresentation } from "./approvalPresentation";
import { approvalOptionLabel } from "./approvalOptions";
import "./approval.css";
import { translate as t } from "./i18n";

export function ApprovalCard({ approval, busy = false, disabled = false, onRespond, children }: {
  approval: PendingApproval; busy?: boolean; disabled?: boolean;
  onRespond?: (optionId: string, answer?: import("@dogcodes/protocol").QuestionAnswer) => void; children?: React.ReactNode;
}) {
  const view = approvalPresentation(approval.commandSummary, approval.toolName);
  const [open, setOpen] = useState(false);
  const menu = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);
  useEffect(() => { if (busy || disabled) setOpen(false); }, [busy, disabled]);
  const primary = approval.options.find(option => option.semanticRole === "approve_once");
  const secondary = approval.options.filter(option => option.semanticRole === "reject" || option.semanticRole === "cancel");
  const more = approval.options.filter(option => option !== primary && !secondary.includes(option));
  const respond = (id: string) => { if (!busy && !disabled) { setOpen(false); onRespond?.(id); } };
  if (approval.question) return <QuestionCard key={approval.approvalId} form={approval.question} busy={busy} disabled={disabled} onAnswer={onRespond ? answer => onRespond("", answer) : undefined}>{children}</QuestionCard>;
  return <article className="approval-card composer-approval" data-approval-id={approval.approvalId} aria-label={t("approvalCard.label")} aria-busy={busy}>
    <header className="approval-heading">
      <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><rect x="3" y="3" width="18" height="18" rx="4"/>{view.kind === "terminal" ? <path d="m7 8 3 3-3 3m6 1h4"/> : <path d="M8 8h8M8 12h8M8 16h5"/>}</svg>
      <span>{view.category}</span><span className="approval-status">{t("approvalCard.status")}</span>
    </header>
    <h2 className="approval-question">{view.question}</h2>
    <ApprovalDetails presentation={view} />
    {children}
    {onRespond && <footer className="approval-actions">
      {secondary.map(option => <button type="button" className="approval-secondary" key={option.optionId} disabled={busy || disabled} onClick={() => respond(option.optionId)}>{approvalOptionLabel(option, approval.options, approval.toolName)}</button>)}
      <div className="approval-action-group" ref={menu} onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); setOpen(false); menu.current?.querySelector<HTMLButtonElement>(".approval-more")?.focus(); } }}>
        {primary && <button type="button" className="approval-primary" disabled={busy || disabled} onClick={() => respond(primary.optionId)}>{busy ? t("question.submitting") : approvalOptionLabel(primary, approval.options, approval.toolName)}</button>}
        {more.length > 0 && <button type="button" className="approval-more" aria-label={t("approvalCard.more")} aria-expanded={open} disabled={busy || disabled} onClick={() => setOpen(!open)}>{!primary && t("approvalCard.choose")}<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><path d="m4 6 4 4 4-4"/></svg></button>}
        {open && <div className="approval-menu" aria-label={t("approvalCard.options")}>{more.map(option => <button type="button" key={option.optionId} disabled={busy || disabled} onClick={() => respond(option.optionId)}>{approvalOptionLabel(option, approval.options, approval.toolName)}</button>)}</div>}
      </div>
    </footer>}
  </article>;
}
