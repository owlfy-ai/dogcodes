import { gitOperations } from "./gitOperations";
import { useEffect, useRef, useState } from "react";
import { FloatingPopover } from "./FloatingPopover";
import { focusDialogTitle } from "./dialogFocus";
import { LoadingStatus } from "./LoadingStatus";
import { reviewGitFailure, type ReviewGitFailure } from "./reviewGitError";
import { InlineError } from "./InlineError";
import type { ReviewRoot } from "./reviewClient";

type RemoteAction = "pull" | "sync";
export type GitRemoteNotice = { rootId: string; action: RemoteAction; busy: boolean; error?: ReviewGitFailure };

export function ReviewGitFailureNotice({ failure, className = "" }: { failure: ReviewGitFailure; className?: string }) {
  return <div className={`review-git-error ${className}`.trim()}><InlineError>{failure.message}</InlineError>{failure.detail && <details><summary>查看错误详情</summary><pre>{failure.detail}</pre></details>}</div>;
}
export function ReviewGitRemoteStatus({ notice }: { notice: GitRemoteNotice | null }) {
  if (!notice) return null;
  if (notice.busy) return <div className="review-remote-status"><LoadingStatus>{notice.action === "pull" ? "正在拉取…" : "正在同步更改…"}</LoadingStatus></div>;
  if (notice.error) return <ReviewGitFailureNotice className="review-remote-status" failure={notice.error} />;
  return <p className="review-remote-status" role="status">{notice.action === "pull" ? "拉取完成" : "同步完成"}</p>;
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
    } catch (cause) { onNotice({ rootId: root.id, action, busy: false, error: reviewGitFailure(cause, action === "pull" ? "拉取失败，请稍后重试。" : "同步失败，请稍后重试。") }); }
    finally { guard.current = false; if (mounted.current) setBusy(false); onBusy(false); onChanged(); }
  };
  return <span ref={host} className="review-git-menu-anchor">
    <button ref={trigger} type="button" className="review-icon-button" aria-label="更多 Git 操作" data-tooltip="更多 Git 操作" aria-haspopup="menu" aria-expanded={open} disabled={disabled || busy} onClick={() => setOpen(value => !value)} onKeyDown={event => { if (event.key === "ArrowDown") { event.preventDefault(); setOpen(true); } }}>
      <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><circle cx="3" cy="8" r="1.2"/><circle cx="8" cy="8" r="1.2"/><circle cx="13" cy="8" r="1.2"/></svg>
    </button>
    {open && <FloatingPopover className="review-file-menu review-git-menu" role="menu" aria-label="Git 操作" placement="bottom" onKeyDown={event => {
      if (!["ArrowDown", "ArrowUp", "Home", "End", "Tab"].includes(event.key)) return;
      if (event.key === "Tab") { setOpen(false); return; }
      event.preventDefault();
      const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
      const current = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (current + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }}>
      <div className="review-git-menu-repository" data-tooltip={root?.path}>{root?.path.split(/[\\/]/).filter(Boolean).at(-1) ?? "未选择 Git 仓库"}</div>
      <button type="button" role="menuitem" disabled={blocked} data-tooltip={root?.git ? "从当前分支的上游拉取更改" : "当前目录未受 Git 管理"} onClick={() => void run("pull")}>拉取</button>
      <button type="button" role="menuitem" disabled={blocked} data-tooltip={root?.git ? "先拉取，再推送本地提交" : "当前目录未受 Git 管理"} onClick={() => { close(); setConfirm(true); }}>同步更改…</button>
    </FloatingPopover>}
    {confirm && <dialog ref={dialog} className="review-confirmation" aria-label="确认同步更改" onCancel={event => { event.preventDefault(); setConfirm(false); }}>
      <h3 tabIndex={-1} data-dialog-title>同步更改？</h3>
      <p>{root?.path}</p><p>将先从当前分支的上游拉取更改，再推送本地提交。出现冲突时会停止，等待处理。</p>
      <button type="button" onClick={() => setConfirm(false)}>取消</button><button type="button" disabled={blocked} onClick={() => void run("sync")}>同步更改</button>
    </dialog>}
  </span>;
}
