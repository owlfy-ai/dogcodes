import { ConfirmDialog } from "./AppDialog";
import { useEffect, useRef, useState } from "react";
import { moveMenuFocus } from "./ContextMenu";
import { FloatingPopover } from "./FloatingPopover";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { reviewClient, type RootChanges, type ReviewChange } from "./reviewClient";
import { translate as t } from "./i18n";
export type FileMenuTarget = { root: RootChanges; file: ReviewChange; snapshotId: string; x: number; y: number; trigger: HTMLButtonElement };
export function ReviewFileMenu({ target, taskId, disabled, onClose, onOpenChanges, onOpenFile, onRevealTree, onChanged, onBusy, remote = false, api = reviewClient }: {
  target: FileMenuTarget; taskId: string; disabled: boolean; remote?: boolean; onClose: () => void; onOpenChanges: () => void;
  onOpenFile: (version: "working" | "head") => void; onRevealTree: () => void; onChanged: () => void; onBusy: (busy: boolean) => void; api?: typeof reviewClient;
}) {
  const [confirm, setConfirm] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const host = useRef<HTMLDivElement>(null), guard = useRef(false);
  const { root, file, snapshotId } = target;
  const gitlink = file.before?.kind === "gitlink" || file.after?.kind === "gitlink";
  const working = ["uncommitted", "unstaged", "session"].includes(root.mode);
  const indexAction = root.root.git && root.mode === "staged" ? "unstage" : root.root.git && ["uncommitted", "unstaged"].includes(root.mode) ? "stage" : null;
  const close = () => { if (!guard.current) onClose(); };
  useEffect(() => {
    host.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
    const outside = (event: PointerEvent) => { if (!confirm && !host.current?.contains(event.target as Node)) close(); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.stopPropagation(); close(); } };
    const resize = () => close();
    document.addEventListener("pointerdown", outside); document.addEventListener("keydown", escape); window.addEventListener("resize", resize);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); window.removeEventListener("resize", resize); };
  }, [confirm]);
  useEffect(() => () => { if (target.trigger.isConnected) target.trigger.focus(); }, [target]);
  const run = async (action: () => Promise<unknown>, fallback: string, changed = false) => {
    if ((changed && disabled) || guard.current) return;
    guard.current = true; setBusy(true); onBusy(true); setError("");
    try { await action(); if (changed) onChanged(); onClose(); }
    catch (cause) { setError(userFacingError(cause, fallback)); }
    finally { guard.current = false; setBusy(false); onBusy(false); }
  };
  const choose = (action: () => void) => { action(); onClose(); };
  const blocked = disabled || busy;
  const noGit = !root.root.git;
  const missing = file.kind === "deleted";
  return <div ref={host}>
    {!confirm && <FloatingPopover className="review-file-menu" role="menu" aria-label={t("fileMenu.label")} point={target} onKeyDown={moveMenuFocus}>
      <button role="menuitem" type="button" onClick={() => choose(onOpenChanges)}>{t("fileMenu.openChanges")}</button>
      <button role="menuitem" type="button" disabled={missing || gitlink} data-tooltip={missing ? t("fileMenu.deleted") : undefined} onClick={() => choose(() => onOpenFile("working"))}>{t("inlineEdit.openFile")}</button>
      <button role="menuitem" type="button" disabled={noGit || gitlink || root.untracked.includes(file.path)} data-tooltip={noGit ? t("gitCommit.notGit") : root.untracked.includes(file.path) ? t("fileMenu.untrackedHead") : undefined} onClick={() => choose(() => onOpenFile("head"))}>{t("fileMenu.openHead")}</button>
      <hr role="separator" />
      <button role="menuitem" type="button" disabled={blocked || !working || gitlink} onClick={() => setConfirm(true)}>{t("fileMenu.discardEllipsis")}</button>
      <button role="menuitem" type="button" disabled={blocked || !indexAction} onClick={() => void run(() => api.index(taskId, snapshotId, root.root.id, file.path, indexAction!), t(indexAction === "unstage" ? "fileMenu.unstageFailed" : "fileMenu.stageFailed"), true)}>{t(indexAction === "unstage" ? "fileMenu.unstage" : "review.stage")}</button>
      <button role="menuitem" type="button" disabled={blocked || noGit || !root.untracked.includes(file.path)} data-tooltip={noGit ? t("gitCommit.notGit") : !root.untracked.includes(file.path) ? t("fileMenu.trackedIgnore") : undefined} onClick={() => void run(() => api.ignore(taskId, snapshotId, root.root.id, file.path), t("fileMenu.ignoreFailed"), true)}>{t("fileMenu.ignore")}</button>
      <hr role="separator" />
      {!remote && <button role="menuitem" type="button" disabled={busy || missing} onClick={() => void run(() => api.reveal(taskId, snapshotId, root.root.id, file.path), t("fileMenu.revealFailed"))}>{t(/Mac/.test(navigator.platform) ? "openFile.reveal.finder" : "openFile.reveal.fileManager")}</button>}
      <button role="menuitem" type="button" disabled={missing} onClick={() => choose(onRevealTree)}>{t("fileMenu.revealTree")}</button>
      {error && <InlineError>{error}</InlineError>}
    </FloatingPopover>}
    {confirm && <ConfirmDialog title={t("fileMenu.discardTitle")} tone="danger" icon="trash" busy={busy} disabled={blocked} wide
      confirmLabel={t(busy ? "gitCommit.processing" : "fileMenu.discard")} onCancel={close}
      onConfirm={() => void run(() => api.discard({ taskId, snapshotId, rootId: root.root.id, path: file.path, confirmed: true }), t("fileMenu.discardFailed"), true)}>
      {[file.previousPath, file.path].filter(Boolean).map(path => <p className="app-dialog-path" key={path}>{root.root.path}/{path}</p>)}
      {file.previousPath && <p>{t("fileMenu.bothPaths")}</p>}
      <p>{t("fileMenu.discardDescription", { base: t(root.mode === "session" ? "fileMenu.base.session" : root.mode === "unstaged" ? "fileMenu.base.index" : "fileMenu.base.head") })}</p>
      {error && <InlineError>{error}</InlineError>}
    </ConfirmDialog>}
  </div>;
}
