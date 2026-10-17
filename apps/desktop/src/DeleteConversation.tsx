import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { TaskSummary } from "@dogcodes/protocol";
import { focusDialogTitle } from "./dialogFocus";
import { userFacingError } from "./userFacingError";

export function DeleteConversation({ task, blocked, onDelete, text = false }: {
  task: TaskSummary;
  blocked: boolean;
  onDelete: (task: TaskSummary) => Promise<void>;
  text?: boolean;
}) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const guard = useRef(false), dialog = useRef<HTMLDialogElement>(null), trigger = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  useEffect(() => {
    if (!confirming) return;
    const element = dialog.current;
    element?.showModal(); focusDialogTitle(element);
    return () => { element?.close(); trigger.current?.focus(); };
  }, [confirming]);
  async function remove() {
    if (guard.current || blocked) return;
    guard.current = true; setBusy(true); setError("");
    try { await onDelete(task); setConfirming(false); }
    catch (cause) { setError(userFacingError(cause, "删除未完成，请刷新会话状态后重试。")); }
    finally { guard.current = false; setBusy(false); }
  }
  return <>
    <button ref={trigger} type="button" data-tooltip={blocked ? "请先停止主任务和审查，再删除会话" : "删除会话"}
      aria-label="删除会话" disabled={blocked} onClick={() => { setError(""); setConfirming(true); }}>
      {text ? "删除" : <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 5h14M7 5V3h6v2M5 5l1 12h8l1-12M8 8v6m4-6v6" /></svg>}
    </button>
    {confirming && createPortal(<dialog ref={dialog} className="turn-undo-dialog" aria-labelledby={titleId}
      onCancel={event => { event.preventDefault(); if (!guard.current) setConfirming(false); }}>
      <h2 id={titleId} tabIndex={-1} data-dialog-title>删除会话？</h2>
      <p>“{task.title}”将被彻底删除，无法恢复。消息、审查报告、历史 Diff 和快照将一并清理。</p>
      <p>项目代码和 Agent 原生会话不会删除。其他会话仍在使用的共享快照会保留。</p>
      {blocked && <p role="alert">会话正在执行主任务或审查，请先停止后再删除。</p>}
      {error && <p role="alert">{error}</p>}
      <footer><button type="button" disabled={busy} onClick={() => setConfirming(false)}>取消</button>
        <button className="turn-undo-confirm" type="button" disabled={busy || blocked} onClick={() => void remove()}>{busy ? "正在删除…" : "彻底删除"}</button></footer>
    </dialog>, document.body)}
  </>;
}
