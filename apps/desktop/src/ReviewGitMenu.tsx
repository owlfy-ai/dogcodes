import { gitOperations } from "./gitOperations";
import { useEffect, useRef, useState } from "react";
import { FloatingPopover } from "./FloatingPopover";
import { focusDialogTitle } from "./dialogFocus";
import { LoadingStatus } from "./LoadingStatus";
import { reviewGitFailure, type ReviewGitFailure } from "./reviewGitError";
import { InlineError } from "./InlineError";
import type { ReviewRoot } from "./reviewClient";
import { translate as t } from "./i18n";

type RemoteAction = "pull" | "sync";
export type GitRemoteNotice = { rootId: string; action: RemoteAction; busy: boolean; error?: ReviewGitFailure };

export function ReviewGitFailureNotice({ failure, className = "" }: { failure: ReviewGitFailure; className?: string }) {
  return <div className={`review-git-error ${className}`.trim()}><InlineError>{failure.message}</InlineError>{failure.detail && <details><summary>{t("gitMenu.errorDetails")}</summary><pre>{failure.detail}</pre></details>}</div>;
}
export function ReviewGitRemoteStatus({ notice }: { notice: GitRemoteNotice | null }) {
  if (!notice) return null;
  if (notice.busy) return <div className="review-remote-status"><LoadingStatus>{t(notice.action === "pull" ? "gitMenu.pulling" : "gitMenu.syncing")}</LoadingStatus></div>;
  if (notice.error) return <ReviewGitFailureNotice className="review-remote-status" failure={notice.error} />;
  return <p className="review-remote-status" role="status">{t(notice.action === "pull" ? "gitMenu.pulled" : "gitMenu.synced")}</p>;
}

export function ReviewGitMenu({ taskId, root, disabled, visible, onBusy, onChanged, onCompleted, onNotice }: {
  taskId: string; root?: ReviewRoot; disabled: boolean; visible: boolean;
  onBusy: (value: boolean) => void; onChanged: () => void; onCompleted: (action: RemoteAction) => void; onNotice: (notice: GitRemoteNotice) => void;
}) {
  const [open, setOpen] = useState(false), [confirm, setConfirm] = useState(false), [busy, setBusy] = useState(false);
  const host = useRef<HTMLSpanElement>(null), trigger = useRef<HTMLButtonElement>(null), dialog = useRef<HTMLDialogElement>(null);
  const guard = useRef(false), mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { if (!visible) { setOpen(false); if (!guard.current) setConfirm(false); } }, [visible]);
  const close = () => { setOpen(false); trigger.current?.focus({ preventScroll: true }); };
  useEffect(() => {
    if (!open) return;
    host.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
    const outside = (event: PointerEvent) => { if (!host.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); } };
    document.addEventListener("pointerdown", outside); document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open]);
  useEffect(() => {
    if (!confirm) return;
    const node = dialog.current; node?.showModal(); focusDialogTitle(node);
    return () => { node?.close(); trigger.current?.focus({ preventScroll: true }); };
  }, [confirm]);
  const blocked = disabled || busy || !root?.git;
  const run = async (action: RemoteAction) => {
    if (blocked || guard.current || !root) return;
    guard.current = true; setBusy(true); setConfirm(false); close(); onBusy(true);
    onNotice({ rootId: root.id, action, busy: true });
    try {
      await gitOperations.run({ taskId, rootId: root.id, action, message: "", stageAll: false });
      onNotice({ rootId: root.id, action, busy: false });
      onCompleted(action);
    } catch (cause) { onNotice({ rootId: root.id, action, busy: false, error: reviewGitFailure(cause, t(action === "pull" ? "gitMenu.pullFailed" : "gitMenu.syncFailed")) }); }
    finally { guard.current = false; if (mounted.current) setBusy(false); onBusy(false); onChanged(); }
  };
  return <span ref={host} className="review-git-menu-anchor">
    <button ref={trigger} type="button" className="review-icon-button" aria-label={t("gitMenu.more")} data-tooltip={t("gitMenu.more")} aria-haspopup="menu" aria-expanded={open} disabled={disabled || busy} onClick={() => setOpen(value => !value)} onKeyDown={event => { if (event.key === "ArrowDown") { event.preventDefault(); setOpen(true); } }}>
      <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><circle cx="3" cy="8" r="1.2"/><circle cx="8" cy="8" r="1.2"/><circle cx="13" cy="8" r="1.2"/></svg>
    </button>
    {open && <FloatingPopover className="review-file-menu review-git-menu" role="menu" aria-label={t("gitMenu.label")} placement="bottom" onKeyDown={event => {
      if (!["ArrowDown", "ArrowUp", "Home", "End", "Tab"].includes(event.key)) return;
      if (event.key === "Tab") { setOpen(false); return; }
      event.preventDefault();
      const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
      const current = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }}>
      <div className="review-git-menu-repository" data-tooltip={root?.path}>{root?.path.split(/[\\/]/).filter(Boolean).at(-1) ?? t("gitMenu.noRepository")}</div>
      <button type="button" role="menuitem" disabled={blocked} data-tooltip={t(root?.git ? "gitMenu.pullTooltip" : "gitCommit.notGit")} onClick={() => void run("pull")}>{t("gitMenu.pull")}</button>
      <button type="button" role="menuitem" disabled={blocked} data-tooltip={t(root?.git ? "gitMenu.syncTooltip" : "gitCommit.notGit")} onClick={() => { close(); setConfirm(true); }}>{t("gitMenu.syncEllipsis")}</button>
    </FloatingPopover>}
    {confirm && <dialog ref={dialog} className="review-confirmation" aria-label={t("gitMenu.confirmSync")} onCancel={event => { event.preventDefault(); setConfirm(false); }}>
      <h3 tabIndex={-1} data-dialog-title>{t("gitMenu.syncTitle")}</h3>
      <p>{root?.path}</p><p>{t("gitMenu.syncDescription")}</p>
      <button type="button" onClick={() => setConfirm(false)}>{t("common.cancel")}</button><button type="button" disabled={blocked} onClick={() => void run("sync")}>{t("gitMenu.sync")}</button>
    </dialog>}
  </span>;
}
