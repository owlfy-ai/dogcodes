import { focusDialogTitle } from "./dialogFocus";
import { useEffect, useRef, useState } from "react";
import { FloatingPopover } from "./FloatingPopover";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { reviewClient, type RootChanges, type ReviewChange } from "./reviewClient";
export type FileMenuTarget = { root: RootChanges; file: ReviewChange; snapshotId: string; x: number; y: number; trigger: HTMLButtonElement };
export function ReviewFileMenu({ target, taskId, disabled, onClose, onOpenChanges, onOpenFile, onRevealTree, onChanged, onBusy, api = reviewClient }: {
  target: FileMenuTarget; taskId: string; disabled: boolean; onClose: () => void; onOpenChanges: () => void;
  onOpenFile: (version: "working" | "head") => void; onRevealTree: () => void; onChanged: () => void; onBusy: (busy: boolean) => void; api?: typeof reviewClient;
}) {
  const [confirm, setConfirm] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const host = useRef<HTMLDivElement>(null), dialog = useRef<HTMLDialogElement>(null), guard = useRef(false);
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
  useEffect(() => {
    if (!confirm) return;
    const node = dialog.current; node?.showModal(); focusDialogTitle(node);
    return () => node?.close();
  }, [confirm]);
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
    {!confirm && <FloatingPopover className="review-file-menu" role="menu" aria-label="文件操作" point={target} onKeyDown={event => {
      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }}>
      <button role="menuitem" type="button" onClick={() => choose(onOpenChanges)}>打开更改</button>
      <button role="menuitem" type="button" disabled={missing || gitlink} data-tooltip={missing ? "工作区中的文件已删除" : undefined} onClick={() => choose(() => onOpenFile("working"))}>打开文件</button>
      <button role="menuitem" type="button" disabled={noGit || gitlink || root.untracked.includes(file.path)} data-tooltip={noGit ? "当前目录未受 Git 管理" : root.untracked.includes(file.path) ? "HEAD 中没有未跟踪文件" : undefined} onClick={() => choose(() => onOpenFile("head"))}>打开文件（HEAD）</button>
      <hr role="separator" />
      <button role="menuitem" type="button" disabled={blocked || !working || gitlink} onClick={() => setConfirm(true)}>丢弃更改…</button>
      <button role="menuitem" type="button" disabled={blocked || !indexAction} onClick={() => void run(() => api.index(taskId, snapshotId, root.root.id, file.path, indexAction!), indexAction === "unstage" ? "取消暂存失败，请稍后重试。" : "暂存更改失败，请稍后重试。", true)}>{indexAction === "unstage" ? "取消暂存更改" : "暂存更改"}</button>
      <button role="menuitem" type="button" disabled={blocked || noGit || !root.untracked.includes(file.path)} data-tooltip={noGit ? "当前目录未受 Git 管理" : !root.untracked.includes(file.path) ? "已跟踪文件不能通过 .gitignore 忽略" : undefined} onClick={() => void run(() => api.ignore(taskId, snapshotId, root.root.id, file.path), "无法加入 .gitignore，请稍后重试。", true)}>加入 .gitignore</button>
      <hr role="separator" />
      <button role="menuitem" type="button" disabled={busy || missing} onClick={() => void run(() => api.reveal(taskId, snapshotId, root.root.id, file.path), "无法在文件管理器中显示此文件。")}>{/Mac/.test(navigator.platform) ? "在 Finder 中显示" : "在文件管理器中显示"}</button>
      <button role="menuitem" type="button" disabled={missing} onClick={() => choose(onRevealTree)}>在文件树中显示</button>
      {error && <InlineError>{error}</InlineError>}
    </FloatingPopover>}
    {confirm && <dialog ref={dialog} className="review-confirmation" role="dialog" aria-modal="true" aria-label="丢弃文件更改" onCancel={event => { event.preventDefault(); close(); }}>
      <h3 tabIndex={-1} data-dialog-title>丢弃此文件的更改？</h3>
      {[file.previousPath, file.path].filter(Boolean).map(path => <p key={path}>{root.root.path}/{path}</p>)}
      {file.previousPath && <p>此操作同时影响以上两个路径。</p>}
      <p>将工作文件恢复为{root.mode === "session" ? "会话初始版本" : root.mode === "unstaged" ? "暂存区版本" : "最近提交版本"}，原先不存在的文件将删除。Git 暂存区不变。</p>
      {error && <InlineError>{error}</InlineError>}
      <button type="button" disabled={busy} onClick={close}>取消</button>
      <button type="button" disabled={blocked} onClick={() => void run(() => api.discard({ taskId, snapshotId, rootId: root.root.id, path: file.path, confirmed: true }), "丢弃更改失败，请稍后重试。", true)}>{busy ? "处理中…" : "丢弃更改"}</button>
    </dialog>}
  </div>;
}
