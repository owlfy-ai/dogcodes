import { focusDialogTitle } from "./dialogFocus";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { reviewClient } from "./reviewClient";
import { gitOperations, GitResultUnknown } from "./gitOperations";
import { reviewGitFailure } from "./reviewGitError";
import { userFacingError } from "./userFacingError";
import { availableAgentExecutors } from "./availableAgentExecutors";
import { readSummaryAgent, readSummaryLanguage } from "./CommitSummarySettings";
import { agentChoiceKey } from "./AgentPickerMenu";
import { turnCommitTargets, type TurnCommitTarget } from "./turnCommit";
import type { ExecutorRef } from "@dogcodes/protocol";
import type { TurnArtifact } from "./turnArtifactCache";

type Phase = "staging" | "summarizing" | "ready" | "committing" | "empty" | "committed";

/** Commits only this turn's files: stages them, has an Agent write the message
 * from exactly that staged diff, then commits (optionally pushes). Closing
 * without committing restores the index entries this dialog staged. */
export function TurnCommitAction({ taskId, data, providerId }: { taskId: string; data: TurnArtifact["data"]; providerId?: string }) {
  const [open, setOpen] = useState(false), [phase, setPhase] = useState<Phase>("staging");
  const [targets, setTargets] = useState<TurnCommitTarget[]>([]), [rootId, setRootId] = useState("");
  const [staged, setStaged] = useState<{ snapshotId: string; files: string[]; extra: number } | null>(null);
  const [message, setMessage] = useState(""), [error, setError] = useState(""), [done, setDone] = useState("");
  const dialog = useRef<HTMLDialogElement>(null), trigger = useRef<HTMLButtonElement>(null);
  const added = useRef<{ rootId: string; paths: string[] } | null>(null), run = useRef(0);
  useEffect(() => {
    if (!open) return;
    dialog.current?.showModal(); focusDialogTitle(dialog.current);
    return () => { dialog.current?.close(); trigger.current?.focus(); };
  }, [open]);

  async function executor(): Promise<ExecutorRef> {
    const available = await availableAgentExecutors();
    const saved = readSummaryAgent();
    const chosen = (saved && available.find(item => agentChoiceKey(item) === agentChoiceKey(saved)))
      ?? available.find(item => item.providerId === providerId) ?? available[0];
    if (!chosen) throw new Error("没有可用于生成提交说明的 Agent");
    return chosen;
  }
  async function stagedSnapshot(target: TurnCommitTarget) {
    const snapshot = await reviewClient.query({ taskId, rootId: target.rootId, mode: "staged" });
    const files = snapshot.roots.find(r => r.root.id === target.rootId)?.files.map(f => f.path) ?? [];
    return { snapshotId: snapshot.id, files };
  }
  async function prepare(target: TurnCommitTarget, id: number) {
    setPhase("staging"); setError(""); setMessage(""); setStaged(null);
    const before = await stagedSnapshot(target);
    const missing = target.paths.filter(path => !before.files.includes(path));
    if (missing.length) {
      const current = await reviewClient.query({ taskId, rootId: target.rootId, mode: "uncommitted" });
      await reviewClient.index(taskId, current.id, target.rootId, missing, "stage");
      added.current = { rootId: target.rootId, paths: missing };
    }
    const after = await stagedSnapshot(target);
    if (id !== run.current) return;
    setStaged({ ...after, extra: after.files.filter(path => !target.paths.includes(path)).length });
    setPhase("summarizing");
    const summary = await invoke<string>("core_review_commit_summary", { taskId, rootId: target.rootId, executor: await executor(), language: readSummaryLanguage() });
    if (id !== run.current) return;
    setMessage(summary.replace(/[\r\n]+/g, " ").trim()); setPhase("ready");
  }
  async function start(target?: TurnCommitTarget) {
    const id = ++run.current;
    try {
      let list = targets;
      if (!target) {
        const snapshot = await reviewClient.query({ taskId, mode: "uncommitted" });
        list = turnCommitTargets(data, snapshot.roots);
        if (id !== run.current) return;
        setTargets(list);
        if (!list.length) { setPhase("empty"); return; }
      }
      const chosen = target ?? list[0];
      setRootId(chosen.rootId);
      await prepare(chosen, id);
    } catch (cause) {
      if (id === run.current) { setError(userFacingError(cause, "准备提交失败，请稍后重试。")); setPhase(message ? "ready" : "empty"); }
    }
  }
  async function restore() {
    const restoring = added.current; added.current = null;
    if (!restoring) return;
    try {
      const snapshot = await reviewClient.query({ taskId, rootId: restoring.rootId, mode: "staged" });
      await reviewClient.index(taskId, snapshot.id, restoring.rootId, restoring.paths, "unstage");
    } catch { /* The 变更 panel still shows the staged files; nothing is lost. */ }
  }
  async function switchRoot(id: string) {
    const target = targets.find(t => t.rootId === id);
    if (!target || id === rootId) return;
    await restore();
    void start(target);
  }
  async function close() {
    run.current++;
    setOpen(false);
    if (phase !== "committed") await restore();
  }
  async function commit(push: boolean) {
    if (!staged || !message.trim() || phase !== "ready") return;
    setPhase("committing"); setError("");
    const operationId = crypto.randomUUID(), nextOperationId = crypto.randomUUID();
    let committed = false;
    try {
      let snapshotId = staged.snapshotId;
      for (let attempt = 0; ; attempt++) {
        try {
          committed = await gitOperations.run({ taskId, rootId, action: "commit", snapshotId, message: message.trim(), stageAll: false }, push ? "commitPush" : "commit", attempt ? {} : { operationId, nextOperationId });
          break;
        } catch (cause) {
          // A stale comparison is rejected before Git runs: re-read and retry once.
          if (attempt || !String(cause instanceof Error ? cause.message : cause).includes("REVIEW_STALE_COMMIT")) throw cause;
          snapshotId = (await stagedSnapshot({ rootId, rootPath: "", paths: [] })).snapshotId;
        }
      }
      if (!committed) throw new Error("没有可提交的更改");
      added.current = null;
      if (push) await gitOperations.advance(taskId, operationId);
      setPhase("committed"); setDone(push ? "已提交并推送" : "已提交");
    } catch (cause) {
      if (committed) { added.current = null; setPhase("committed"); setDone("已提交"); }
      else setPhase("ready");
      setError(cause instanceof GitResultUnknown
        ? "提交结果待核实，请在「变更」面板核实，不要重复提交。"
        : committed ? reviewGitFailure(cause, "提交已完成，推送失败。请在「变更」面板重试推送。", "提交已完成，推送失败：").message
        : reviewGitFailure(cause, "提交失败，请稍后重试。").message);
    }
  }

  const busy = phase === "staging" || phase === "summarizing" || phase === "committing";
  const status = phase === "staging" ? "正在暂存本轮文件…" : phase === "summarizing" ? "正在生成提交说明…" : phase === "committing" ? "正在提交…" : "";
  return <>
    <button ref={trigger} className="turn-summary-undo turn-summary-commit" type="button" disabled={!!done}
      aria-label={done || "生成提交说明并提交本轮修改"}
      onClick={event => { event.preventDefault(); event.stopPropagation(); setOpen(true); void start(); }}>
      {!done && <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" aria-hidden="true"><circle cx="8" cy="8" r="3" /><path d="M1.5 8H5m6 0h3.5" /></svg>}
      {done || "提交"}
    </button>
    {open && createPortal(<dialog ref={dialog} className="turn-undo-dialog turn-commit-dialog" aria-labelledby="turn-commit-title" aria-busy={busy}
      onClick={event => event.stopPropagation()} onCancel={event => { event.preventDefault(); if (phase !== "committing") void close(); }}>
      <h2 id="turn-commit-title" tabIndex={-1} data-dialog-title>提交本轮修改</h2>
      {targets.length > 1 && <label className="turn-commit-root">仓库
        <select value={rootId} disabled={busy} onChange={event => void switchRoot(event.target.value)}>
          {targets.map(t => <option key={t.rootId} value={t.rootId}>{t.rootPath.split("/").pop() || t.rootPath}</option>)}
        </select></label>}
      {phase === "empty" && !error && <p>本轮修改已没有未提交的内容（可能已提交或已撤销）。</p>}
      {staged && <p className="turn-undo-hint">将提交 {staged.files.length} 个文件{staged.extra ? `，其中 ${staged.extra} 个是此前已暂存的其他更改` : ""}。</p>}
      {(phase === "ready" || phase === "committing" || phase === "committed" || (phase === "summarizing" && message)) &&
        <textarea className="turn-commit-message" aria-label="提交说明" rows={3} value={message} disabled={busy || phase === "committed"} onChange={event => setMessage(event.target.value)} />}
      {status && <p role="status">{status}</p>}
      {done && <p role="status">{done}</p>}
      {error && <p role="alert">{error}</p>}
      <footer>
        <button type="button" disabled={phase === "committing"} onClick={() => void close()}>{phase === "committed" ? "完成" : "取消"}</button>
        {phase !== "committed" && phase !== "empty" && <>
          <button type="button" disabled={busy || !staged} onClick={() => { const t = targets.find(x => x.rootId === rootId); if (t) { const id = ++run.current; void prepare(t, id).catch(cause => { if (id === run.current) { setError(userFacingError(cause, "生成提交说明失败，请稍后重试。")); setPhase("ready"); } }); } }}>重新生成</button>
          <button type="button" disabled={busy || !message.trim()} onClick={() => void commit(false)}>提交</button>
          <button className="turn-commit-confirm" type="button" disabled={busy || !message.trim()} onClick={() => void commit(true)}>提交并推送</button>
        </>}
      </footer>
    </dialog>, document.body)}
  </>;
}
