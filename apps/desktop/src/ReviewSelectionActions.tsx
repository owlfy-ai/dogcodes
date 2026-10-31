import { useEffect, useRef, useState } from "react";
import { FloatingPopover } from "./FloatingPopover";
import { focusDialogTitle } from "./dialogFocus";
import { directoryName } from "./ReviewFileTree";
import type { FileMenuTarget } from "./ReviewFileMenu";
import { reviewClient, type ReviewSnapshot } from "./reviewClient";
import { reviewFileKey } from "./useReviewFileSelection";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

type BatchAction = "stage" | "unstage" | "discard" | "ignore";
export function ReviewSelectionActions({ taskId, snapshot, selected, disabled, onBusy, onChanged, onRemove, menu, onCloseMenu, api }: {
  taskId: string; snapshot: ReviewSnapshot; selected: Set<string>; disabled: boolean; api: typeof reviewClient;
  onBusy: (busy: boolean) => void; onChanged: () => void; onRemove: (keys: string[]) => void;
  menu: FileMenuTarget | null; onCloseMenu: () => void;
}) {
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  const guard = useRef(false), menuHost = useRef<HTMLDivElement>(null), dialog = useRef<HTMLDialogElement>(null);
  const groups = snapshot.roots.map(root => ({ root, files: root.files.filter(file => selected.has(reviewFileKey(root.root.id, file.path))) })).filter(group => group.files.length);
  const count = groups.reduce((total, group) => total + group.files.length, 0);
  const [confirmation, setConfirmation] = useState<{ groups: typeof groups; snapshotId: string; count: number } | null>(null);
  const working = groups.length > 0 && groups.every(({ root }) => !root.error && ["uncommitted", "unstaged", "session"].includes(root.mode));
  const action = groups.length && groups.every(({ root }) => root.root.git && !root.error && ["uncommitted", "unstaged"].includes(root.mode)) ? "stage"
    : groups.length && groups.every(({ root }) => root.root.git && !root.error && root.mode === "staged") ? "unstage" : null;
  const discardAllowed = working && groups.every(({ files }) => files.every(file => file.before?.kind !== "gitlink" && file.after?.kind !== "gitlink"));
  const ignoreAllowed = working && groups.every(({ root, files }) => root.root.git && files.every(file => root.untracked.includes(file.path) && !file.previousPath));
  const label = t(action === "unstage" ? "selection.unstage" : "selection.stage");
  const blocked = disabled || busy;
  const activeMenu = count > 1 ? menu : null;
  const closeMenu = onCloseMenu;
  useEffect(() => {
    if (!activeMenu) return;
    (menuHost.current?.querySelector<HTMLButtonElement>("button:not(:disabled)") ?? menuHost.current)?.focus();
    const outside = (event: PointerEvent) => { if (!menuHost.current?.contains(event.target as Node) && !activeMenu.trigger.contains(event.target as Node)) closeMenu(); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.stopPropagation(); closeMenu(); } };
    document.addEventListener("pointerdown", outside); document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); if (activeMenu.trigger.isConnected) activeMenu.trigger.focus(); };
  }, [activeMenu, closeMenu]);
  useEffect(() => {
    if (!confirmation) return;
    const node = dialog.current; node?.showModal(); focusDialogTitle(node);
    return () => node?.close();
  }, [confirmation]);
  const allowed = (operation: BatchAction) => operation === "discard" ? discardAllowed : operation === "ignore" ? ignoreAllowed : operation === action;
  const run = async (operation: BatchAction) => {
    if (blocked || guard.current || !allowed(operation) || operation === "discard" && !confirmation) return;
    const chosen = operation === "discard" ? confirmation! : { groups, snapshotId: snapshot.id, count };
    guard.current = true; setBusy(true); onBusy(true); setError(""); closeMenu();
    let completed = 0;
    try {
      for (const { root, files } of chosen.groups) {
        const paths = files.map(file => file.path);
        try {
          const result = operation === "stage" || operation === "unstage"
            ? (await api.index(taskId, chosen.snapshotId, root.root.id, paths, operation), { completed: paths, error: null })
            : await api.batchFiles({ taskId, snapshotId: chosen.snapshotId, rootId: root.root.id, paths, action: operation, ...(operation === "discard" ? { confirmed: true } : {}) });
          completed += result.completed.length;
          onRemove(result.completed.map(path => reviewFileKey(root.root.id, path)));
          if (result.error) throw new Error(result.error);
        } catch (cause) { throw new Error(t("quota.labelValue", { label: directoryName(root.root.path), value: userFacingError(cause, t("selection.failed")) })); }
      }
    } catch (cause) {
      // Per-root failures are already cleaned and prefixed with their directory above.
      setError(`${completed ? t("selection.partial", { count: completed }) : t("selection.incomplete")}${cause instanceof Error ? cause.message : userFacingError(cause, t("selection.failed"))}`);
    } finally {
      guard.current = false; setBusy(false); setConfirmation(null); onBusy(false); onChanged();
    }
  };
  const confirmDiscard = () => {
    if (blocked || !discardAllowed) return;
    setError(""); closeMenu(); setConfirmation({ groups, snapshotId: snapshot.id, count });
  };
  return <>
    {error && <InlineError className="review-selection-error">{error}</InlineError>}
    {activeMenu && !confirmation && <div ref={menuHost} tabIndex={-1}><FloatingPopover className="review-file-menu" role="menu" aria-label={t("selection.menu")} point={activeMenu} onKeyDown={event => {
      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }}>
      <button role="menuitem" type="button" disabled={blocked || !action} onClick={() => action && void run(action)}>{t("selection.withCount", { label, count })}</button>
      <button role="menuitem" type="button" disabled={blocked || !ignoreAllowed} data-tooltip={!ignoreAllowed ? t("selection.ignoreTooltip") : undefined} onClick={() => void run("ignore")}>{t("selection.withCount", { label: t("fileMenu.ignore"), count })}</button>
      <hr role="separator" />
      <button role="menuitem" type="button" disabled={blocked || !discardAllowed} data-tooltip={!discardAllowed ? t("selection.discardTooltip") : undefined} onClick={confirmDiscard}>{t("selection.discardEllipsis")}</button>
    </FloatingPopover></div>}
    {confirmation && <dialog ref={dialog} className="review-confirmation" aria-label={t("selection.discardLabel")} aria-modal="true" onCancel={event => { event.preventDefault(); if (!guard.current) setConfirmation(null); }}>
      <h3 tabIndex={-1} data-dialog-title>{t("selection.discardTitle", { count: confirmation.count })}</h3>
      <p>{t("selection.discardDescription")}</p>
      <div className="review-batch-file-list">{confirmation.groups.map(({ root, files }) => <section key={root.root.id}><strong data-tooltip={root.root.path}>{root.root.path} · {t(root.mode === "session" ? "fileMenu.base.session" : root.mode === "unstaged" ? "fileMenu.base.index" : "fileMenu.base.head")}</strong><ul>{files.map(file => <li key={file.path}>{file.previousPath ? `${file.previousPath} → ${file.path}` : file.path}</li>)}</ul></section>)}</div>
      <button type="button" disabled={busy} onClick={() => setConfirmation(null)}>{t("common.cancel")}</button>
      <button type="button" className="review-discard-confirm" disabled={blocked} onClick={() => void run("discard")}>{busy ? t("gitCommit.processing") : t("selection.discardConfirm", { count: confirmation.count })}</button>
    </dialog>}
  </>;
}
