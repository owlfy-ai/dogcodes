import { gitOperations, GitResultUnknown, GitPushFailed } from "./gitOperations";
import { reviewGitError } from "./reviewGitError";
import { reviewGitFailure, type ReviewGitFailure } from "./reviewGitError";
import { ReviewGitFailureNotice } from "./ReviewGitMenu";
import { userFacingError } from "./userFacingError";
import { readSummaryLanguage, summaryLanguageLabel, COMMIT_LANGUAGE_KEY, SUMMARY_LANGUAGES, type SummaryLanguage } from "./CommitSummarySettings";
import { CommitSummaryPicker, useCommitSummaryChoice, type SummaryChoice } from "./CommitSummaryPicker";
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FloatingPopover } from "./FloatingPopover";
import { readSkipStageAllConfirmation, setSkipStageAllConfirmation, useSkipStageAllConfirmation } from "./gitCommitPreferences";
import { providerLabel } from "./agentPresentation";
import type { ReviewRoot } from "./reviewClient";
import { translate as t } from "./i18n";

type Action = "commit" | "commitPush";
export function ReviewGitActions({ taskId, root, snapshotId, syncedRevision = 0, pulledRevision = 0, hasChanges, disabled, onChanged, onBusy }: {
  taskId: string; root?: ReviewRoot; snapshotId: string; syncedRevision?: number; pulledRevision?: number; hasChanges: boolean; disabled: boolean; onChanged: () => void; onBusy?: (busy: boolean) => void;
}) {
  const [action, setAction] = useState<Action>("commit");
  const [stageAll, setStageAll] = useState<string | null>(null);
  const [rememberStageAll, setRememberStageAll] = useState(false);
  const skipStageAllConfirmation = useSkipStageAllConfirmation();
  const [message, setMessage] = useState("");
  const [summarizing, setSummarizing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [pushPending, setPushPending] = useState(false);
  const [error, setError] = useState<ReviewGitFailure | null>(null);
  const [language, setLanguage] = useState<SummaryLanguage>(readSummaryLanguage);
  const [langMenu, setLangMenu] = useState(false);
  const choice = useCommitSummaryChoice(taskId, { onError: cause => setError({ message: userFacingError(cause, t("gitCommit.agentsFailed")) }) });
  const { selected, loadingAgents } = choice;
  useEffect(() => { if (pulledRevision) setError(null); }, [pulledRevision]);
  useEffect(() => { if (syncedRevision) { setPushPending(false); setError(null); } }, [syncedRevision]);
  const section = useRef<HTMLElement>(null), langTrigger = useRef<HTMLButtonElement>(null);
  const workflowCommit = useRef<string | null>(null);
  const mounted = useRef(true), guard = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    if (!langMenu) return;
    const close = (event: PointerEvent) => { if (!section.current?.contains(event.target as Node)) setLangMenu(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.stopPropagation(); setLangMenu(false); langTrigger.current?.focus(); } };
    document.addEventListener("pointerdown", close); document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", close); document.removeEventListener("keydown", escape); };
  }, [langMenu]);
  const reason = !root?.git ? t("gitCommit.notGit") : disabled ? t("gitCommit.waitOperation") : "";
  const locked = !!reason || busy || summarizing;
  const submit = async (kind: Action, confirmStageAll = false) => {
    if (!root?.git || guard.current || locked || (!pushPending && !message.trim())) return;
    guard.current = true; setAction(kind); setBusy(true); onBusy?.(true); setError(null); choice.close();
    const pushOperationId = crypto.randomUUID();
    if (!pushPending) workflowCommit.current = crypto.randomUUID();
    let committed = pushPending;
    let refresh = pushPending;
    try {
      if (!committed) {
        const completed = await gitOperations.run({ taskId, rootId: root.id, action: "commit", snapshotId: confirmStageAll ? stageAll : snapshotId, message: message.trim(), stageAll: confirmStageAll || readSkipStageAllConfirmation() }, kind, { operationId: workflowCommit.current!, nextOperationId: pushOperationId });
        if (!completed) { if (mounted.current) { setRememberStageAll(false); setStageAll(snapshotId); } return; }
        if (confirmStageAll && rememberStageAll) setSkipStageAllConfirmation(true);
        committed = true; refresh = true;
        if (mounted.current) { setStageAll(null); setMessage(""); setPushPending(kind === "commitPush"); }
      }
      if (kind === "commitPush" || pushPending) {
        await gitOperations.advance(taskId, workflowCommit.current!);
        if (mounted.current) setPushPending(false);
      }
    } catch (cause) {
      refresh = true;
      if (mounted.current) {
        setStageAll(null);
        if (cause instanceof GitResultUnknown && committed) {
          // Preserve the original operation identity while its result is uncertain.
          setPushPending(false); setError({ message: t("gitCommit.pushUnverified"), detail: cause.detail ?? cause.message });
        } else if (cause instanceof GitResultUnknown) {
          // Not a failure from Git: a commit may have happened, so say so rather than "failed".
          setError({ message: cause.message, ...(cause.detail ? { detail: cause.detail } : {}) });
        } else {
          if (cause instanceof GitPushFailed) setPushPending(false);
          setError(committed ? reviewGitFailure(cause, t("gitCommit.pushFailed"), t("gitCommit.pushFailedPrefix")) : reviewGitFailure(cause, t("gitCommit.commitFailed")));
        }
      }
    } finally {
      guard.current = false; onBusy?.(false);
      // Hooks and external Git tools can change disk state even on failure.
      if (refresh) onChanged();
      if (mounted.current) setBusy(false);
    }
  };
  const summarize = async ({ executor, model }: SummaryChoice) => {
    if (!root?.git || guard.current || locked || pushPending) return;
    guard.current = true; setSummarizing(true); onBusy?.(true); choice.close(); setLangMenu(false); setError(null);
    try {
      const summary = await invoke<string>("core_review_commit_summary", { taskId, rootId: root.id, executor, language, model });
      if (mounted.current) { setMessage(summary.replace(/[\r\n]+/g, " ").trim()); setStageAll(null); }
    } catch (cause) { if (mounted.current) setError({ message: userFacingError(cause, t("gitCommit.generateFailed")) }); }
    finally { guard.current = false; onBusy?.(false); if (mounted.current) setSummarizing(false); }
  };
  const chooseLanguage = (id: SummaryLanguage) => {
    setLanguage(id); setLangMenu(false);
    try { localStorage.setItem(COMMIT_LANGUAGE_KEY, id); } catch { /* Selection still works for this panel. */ }
    langTrigger.current?.focus();
  };
  const generate = async () => {
    if (guard.current || locked || loadingAgents) return;
    setLangMenu(false); setError(null);
    let resolved: SummaryChoice | null;
    try { resolved = await choice.resolve(); }
    catch (cause) { if (mounted.current) setError({ message: userFacingError(cause, t("gitCommit.agentsFailed")) }); return; }
    if (!mounted.current) return;
    if (resolved) void summarize(resolved); else choice.setAgentMenu(true, true);
  };
  // A successful commit can empty the diff while a push is still pending.
  // Retain its retry controls until that operation finishes.
  if (!root?.git || (!hasChanges && !pushPending && !busy && !summarizing && !error)) return null;
  const agentName = selected ? providerLabel(selected.providerId) : t("gitCommit.selectAgent");
  const empty = !message.trim();
  return <section className="review-commit" ref={section} aria-label={t("gitCommit.title")} aria-busy={busy || summarizing}>
    {(hasChanges || pushPending) && <><div className="review-commit-input">
      <input type="text" aria-label={t("gitCommit.message")} placeholder={t("gitCommit.messagePlaceholder")} value={message} disabled={locked || pushPending}
        onChange={event => { setMessage(event.target.value); setStageAll(null); }}
        onKeyDown={event => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter" && !event.nativeEvent.isComposing) { event.preventDefault(); void submit(event.shiftKey ? "commitPush" : "commit"); } }} />
      <div className="review-commit-tools">
        <CommitSummaryPicker choice={choice} disabled={locked || pushPending} reason={reason} onGenerate={resolved => void summarize(resolved)} />
        <div className="review-generate-group">
          <button type="button" className="review-generate-message" aria-label={t("review.generateCommit")} data-tooltip={reason || (selected ? t("gitCommit.generateTooltip", { agent: agentName, language: summaryLanguageLabel(language) }) : t("gitCommit.selectAgentFirst"))} disabled={locked || pushPending || loadingAgents} onClick={() => void generate()}>
            <svg className={summarizing ? "spinning" : undefined} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinejoin="round" aria-hidden="true"><path d="m10 3 2.4 6.6L19 12l-6.6 2.4L10 21l-2.4-6.6L1 12l6.6-2.4L10 3Zm10-2 1 3 3 1-3 1-1 3-1-3-3-1 3-1 1-3Z" /></svg>
            <span>{t(summarizing ? "gitCommit.generating" : "gitCommit.generate")}</span>
          </button>
          <button ref={langTrigger} type="button" className="review-generate-language" aria-label={t("review.selectCommitLanguage")} data-tooltip={reason || t("gitCommit.languageTooltip", { language: summaryLanguageLabel(language) })} aria-expanded={langMenu} aria-haspopup="dialog" disabled={locked || pushPending || loadingAgents} onClick={() => { choice.close(); setLangMenu(!langMenu); }}>
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m4.5 6.5 3.5 3.5 3.5-3.5" /></svg>
          </button>
          {langMenu && <FloatingPopover className="agent-picker-menu review-language-menu" role="dialog" aria-label={t("review.commitLanguage")} placement="bottom">
            {SUMMARY_LANGUAGES.map(option => {
              const chosen = option.id === language;
              return <button type="button" key={option.id} className={`review-language-option${chosen ? " active" : ""}`} aria-pressed={chosen} onClick={() => chooseLanguage(option.id)}>
                <span>{option.label}</span>
                {chosen && <i>✓</i>}
              </button>;
            })}
          </FloatingPopover>}
        </div>
      </div>
    </div>
    <div className="review-commit-buttons">
      {pushPending ? <button type="button" className="review-commit-action review-commit-push" disabled={locked} data-tooltip={reason || t("gitCommit.retryPushTooltip")} onClick={() => void submit("commitPush")}>
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M3 3V1.5h10V3M8 14V5M4.5 8.5 8 5l3.5 3.5" /></svg>
        <span>{t(busy ? "gitCommit.pushing" : "gitCommit.retryPush")}</span>
      </button> : <>
        <button type="button" className="review-commit-action" disabled={locked || empty} data-tooltip={reason || t(skipStageAllConfirmation ? "gitCommit.commitTooltipAuto" : "gitCommit.commitTooltipConfirm")} onClick={() => void submit("commit")}>
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true"><circle cx="8" cy="8" r="3" /><path d="M1.5 8H5m6 0h3.5" /></svg>
          <span>{t(busy && action === "commit" ? "gitCommit.processing" : "gitCommit.commit")}</span>
        </button>
        <button type="button" className="review-commit-action review-commit-push" disabled={locked || empty} data-tooltip={reason || t("gitCommit.commitPushTooltip")} onClick={() => void submit("commitPush")}>
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M3 3V1.5h10V3M8 14V5M4.5 8.5 8 5l3.5 3.5" /></svg>
          <span>{t(busy && action === "commitPush" ? "gitCommit.processing" : "gitCommit.commitPush")}</span>
        </button>
      </>}
    </div>
    </>}
    {stageAll && <div className="review-commit-confirm" role="group" aria-label={t("gitCommit.confirmStageAll")}
      onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); setStageAll(null); } }}>
      <div className="review-commit-confirm-text">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true"><circle cx="8" cy="8" r="6.25" /><path d="M8 4.75v3.75M8 11h.01" /></svg>
        <p><strong>{t("gitCommit.nothingStaged")}</strong>{t(action === "commitPush" ? "gitCommit.stageAllThenPush" : "gitCommit.stageAllThenCommit")}</p>
      </div>
      <div className="review-commit-confirm-actions">
        <label className="review-commit-confirm-remember">
          <input type="checkbox" checked={rememberStageAll} disabled={locked} onChange={event => setRememberStageAll(event.target.checked)} />
          <span>{t("gitCommit.dontShowAgain")}</span>
        </label>
        <button type="button" className="review-commit-cancel" disabled={locked} onClick={() => setStageAll(null)}>{t("common.cancel")}</button>
        <button type="button" className="review-commit-primary" autoFocus disabled={locked} onClick={() => void submit(action, true)}>{t(action === "commitPush" ? "gitCommit.stageCommitPush" : "gitCommit.stageCommit")}</button>
      </div>
    </div>}
    {reason && !busy && !summarizing && <p className="review-commit-hint">{reason}</p>}
    {error && <ReviewGitFailureNotice failure={error} />}
  </section>;
}
