import { useEffect, useRef, useState } from "react";
import { FloatingPopover } from "./FloatingPopover";
import { focusDialogTitle } from "./dialogFocus";
import { directoryName } from "./ReviewFileTree";
import type { FileMenuTarget } from "./ReviewFileMenu";
import { reviewClient, type ReviewSnapshot } from "./reviewClient";
import { reviewFileKey } from "./useReviewFileSelection";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";

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
  const label = action === "unstage" ? "取消暂存所选" : "暂存所选";
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
        } catch (cause) { throw new Error(`${directoryName(root.root.path)}：${userFacingError(cause, "文件操作失败，请稍后重试。")}`); }
      }
    } catch (cause) {
      // Per-root failures are already cleaned and prefixed with their directory above.
      setError(`${completed ? `已完成 ${completed} 个文件，其余未完成。` : "操作未完成。"}${cause instanceof Error ? cause.message : userFacingError(cause, "文件操作失败，请稍后重试。")}`);
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
    {activeMenu && !confirmation && <div ref={menuHost} tabIndex={-1}><FloatingPopover className="review-file-menu" role="menu" aria-label="所选文件操作" point={activeMenu} onKeyDown={event => {
      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }}>
      <button role="menuitem" type="button" disabled={blocked || !action} onClick={() => action && void run(action)}>{label}（{count} 个文件）</button>
      <button role="menuitem" type="button" disabled={blocked || !ignoreAllowed} data-tooltip={!ignoreAllowed ? "仅支持未跟踪文件；已跟踪文件无法通过 .gitignore 忽略" : undefined} onClick={() => void run("ignore")}>加入 .gitignore（{count} 个文件）</button>
      <hr role="separator" />
      <button role="menuitem" type="button" disabled={blocked || !discardAllowed} data-tooltip={!discardAllowed ? "仅支持工作区更改，不支持历史、已暂存视图和子模块" : undefined} onClick={confirmDiscard}>丢弃所选更改…</button>
    </FloatingPopover></div>}
    {confirmation && <dialog ref={dialog} className="review-confirmation" aria-label="丢弃所选文件更改" aria-modal="true" onCancel={event => { event.preventDefault(); if (!guard.current) setConfirmation(null); }}>
      <h3 tabIndex={-1} data-dialog-title>丢弃所选 {confirmation.count} 个文件的更改？</h3>
      <p>工作文件将恢复到以下比较基准。新增文件将删除，重命名将同时恢复原路径。Git 暂存区保持不变。</p>
      <div className="review-batch-file-list">{confirmation.groups.map(({ root, files }) => <section key={root.root.id}><strong data-tooltip={root.root.path}>{root.root.path} · {root.mode === "session" ? "会话初始版本" : root.mode === "unstaged" ? "暂存区版本" : "最近提交版本"}</strong><ul>{files.map(file => <li key={file.path}>{file.previousPath ? `${file.previousPath} → ${file.path}` : file.path}</li>)}</ul></section>)}</div>
      <button type="button" disabled={busy} onClick={() => setConfirmation(null)}>取消</button>
      <button type="button" className="review-discard-confirm" disabled={blocked} onClick={() => void run("discard")}>{busy ? "处理中…" : `丢弃 ${confirmation.count} 个文件的更改`}</button>
    </dialog>}
  </>;
}
