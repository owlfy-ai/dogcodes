import { useEffect, useState, useSyncExternalStore } from "react";
import { gitOperations, type PendingGit } from "./gitOperations";
import "./gitOperationRecovery.css";
import { subscribeReviewChanges } from "./reviewWatch";
const labels = { commit: "提交", push: "推送", pull: "拉取", sync: "同步" };
export function GitOperationRecovery({ taskId, visible = true, onChanged }: { taskId: string; visible?: boolean; onChanged: (settled?: boolean) => void }) {
  const raw = useSyncExternalStore(gitOperations.subscribe, () => gitOperations.snapshot(taskId), () => "[]");
  const parsed = JSON.parse(raw) as PendingGit[] | { error: string };
  const items = Array.isArray(parsed) ? parsed : [];
  const [error, setError] = useState(""), [busy, setBusy] = useState<string | null>(null);
  const ids = items.map(item => item.request.operationId).join(",");
  useEffect(() => {
    if (!visible || !ids) return;
    let active = true;
    const refresh = () => { void Promise.all(items.map(item => gitOperations.check(item))).catch(cause => { if (active) setError(String(cause)); }); };
    refresh();
    const stop = subscribeReviewChanges(taskId, refresh);
    return () => { active = false; stop(); };
    // Query after opening, identity changes, or service invalidations; never auto-resend.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [taskId, visible, ids]);
  if (!Array.isArray(parsed)) return <p role="alert">{parsed.error}</p>;
  if (!items.length) return null;
  const run = async (item: PendingGit, action: () => Promise<unknown>) => {
    setBusy(item.request.operationId); setError("");
    try {
      const result = await action();
      const receipt = result && typeof result === "object" && "outcome" in result ? result as { outcome: { status: string } } : null;
      onChanged(typeof result === "boolean" || receipt?.outcome.status === "succeeded" || receipt?.outcome.status === "failed");
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(null); }
  };
  return <section className="git-operation-recovery" aria-label="待核实的 Git 操作">
    {items.map(item => {
      const state = item.receipt?.outcome;
      const completed = state?.status === "succeeded";
      const targetError = completed ? state.targetError : undefined;
      const needsPush = completed && !targetError && !item.receipt?.closed && state.completed && item.intent === "commitPush" && item.request.action === "commit";
      return <div key={item.request.operationId}>
        <p><strong>{item.committed ? "提交已完成，" : ""}{labels[item.request.action]}</strong>{item.request.message && ` · ${item.request.message}`}</p>
        <p role="status">{targetError ? `提交已完成，推送目标待核实：${targetError}` : needsPush ? "提交已完成，尚未推送。" : completed ? state.completed ? "操作已完成。" : "未执行提交，需要先确认暂存范围。" : state?.status === "failed" ? state.message : state?.status === "running" ? "后台仍在执行，可稍后再次核实。" : item.receipt === null ? "后台尚未找到已受理的记录，可重发原请求。" : "结果尚未确定，请先核实；不要重复发起操作。"}</p>
        <div className="git-operation-recovery-actions">
          <button type="button" disabled={!!busy || item.sending} onClick={() => void run(item, () => gitOperations.check(item))}>核实结果</button>
          {item.receipt === null && <button type="button" disabled={!!busy || item.sending} onClick={() => void run(item, () => gitOperations.resend(item))}>重发原请求</button>}
          {needsPush && <button type="button" disabled={!!busy || item.sending} onClick={() => void run(item, () => gitOperations.continuePush(item))}>继续推送</button>}
          {!item.receipt?.closed && state?.status === "failed" && item.committed && item.request.action === "push" && item.retryOperationId && <button type="button" disabled={!!busy || item.sending} onClick={() => void run(item, () => gitOperations.retryPush(item))}>重试推送</button>}
          {(state?.status === "succeeded" || state?.status === "failed" || state?.status === "unknown") && <button type="button" disabled={!!busy || item.sending} onClick={() => void run(item, async () => { await gitOperations.acknowledge(item); return true; })}>{completed && !needsPush ? "完成" : "已核对仓库，结束此次操作"}</button>}
        </div>
      </div>;
    })}
    {error && <p role="alert">{error}</p>}
  </section>;
}
