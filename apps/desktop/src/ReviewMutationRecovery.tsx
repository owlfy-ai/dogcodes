import { useEffect, useState, useSyncExternalStore } from "react";
import { mutationArguments, partialMutationResult, reviewMutations, type PendingMutation } from "./reviewMutations";
import "./gitOperationRecovery.css";
import { subscribeReviewChanges } from "./reviewWatch";

function label(item: PendingMutation) {
  const args = mutationArguments(item);
  if (item.request.command === "core_turn_undo") return "撤销本轮文件修改";
  if (item.request.command === "core_review_index") return args.action === "stage" ? "暂存" : "取消暂存";
  if (item.request.command === "core_review_batch_files") return args.action === "discard" ? "批量丢弃" : "批量忽略";
  return item.request.command === "core_review_ignore" ? "忽略" : "丢弃";
}
export function ReviewMutationRecovery({ taskId, artifactId, visible = true, onChanged }: { taskId: string; artifactId?: string; visible?: boolean; onChanged: () => void }) {
  const raw = useSyncExternalStore(reviewMutations.subscribe, () => reviewMutations.snapshot(taskId), () => "[]");
  const parsed = JSON.parse(raw) as PendingMutation[] | { error: string };
  const items = Array.isArray(parsed) ? parsed.filter(item => !artifactId || (item.request.command === "core_turn_undo" && mutationArguments(item).artifactId === artifactId)) : [];
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  const ids = items.map(item => item.request.operationId).join(",");
  useEffect(() => {
    if (!visible || !ids) return;
    let active = true;
    const refresh = () => { void Promise.all(items.map(item => reviewMutations.check(item))).catch(cause => { if (active) setError(String(cause)); }); };
    refresh();
    const stop = subscribeReviewChanges(taskId, refresh);
    return () => { active = false; stop(); };
    // Query on opening, identity changes and service invalidations. Never auto-replay.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [taskId, artifactId, visible, ids]);
  if (!Array.isArray(parsed)) return <p role="alert">{parsed.error}</p>;
  if (!items.length) return null;
  const run = async (action: () => unknown) => {
    setBusy(true); setError("");
    try { await action(); onChanged(); } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  return <section className="git-operation-recovery" aria-label="待核实的文件操作">
    {items.map(item => {
      const state = item.receipt?.outcome, args = mutationArguments(item);
      const paths = Array.isArray(args.paths) ? args.paths.join("、") : args.path;
      const partial = state?.status === "succeeded" ? partialMutationResult(state.result) : null;
      const terminal = state?.status === "succeeded" || state?.status === "failed" || state?.status === "unknown";
      return <div key={item.request.operationId}>
        <p><strong>{label(item)}</strong>{typeof paths === "string" && ` · ${paths}`}</p>
        <p role="status">{partial ? `已完成 ${partial.completed.length} 个文件，部分未完成：${partial.error}` : state?.status === "succeeded" ? "操作已完成。" : state?.status === "failed" ? `操作未全部完成：${state.message}；请刷新核对文件。` : state?.status === "running" ? "后台仍在执行，可稍后再次核实。" : item.receipt === null ? "后台尚未找到已受理记录，可重发原请求。" : "结果尚未确定，请先核实文件及恢复记录，不要重复发起操作。"}</p>
        {partial && partial.completed.length > 0 && <details><summary>已完成的文件</summary><ul>{partial.completed.map(path => <li key={path}>{path}</li>)}</ul></details>}
        <div className="git-operation-recovery-actions">
          <button type="button" disabled={busy || item.sending} onClick={() => void run(() => reviewMutations.check(item))}>核实结果</button>
          {item.receipt === null && <button type="button" disabled={busy || item.sending} onClick={() => void run(() => reviewMutations.resend(item))}>重发原请求</button>}
          {terminal && <button type="button" disabled={busy || item.sending} onClick={() => void run(() => reviewMutations.acknowledge(item))}>{state?.status === "succeeded" && !partial ? "完成" : "已核对文件，结束此次操作"}</button>}
        </div>
      </div>;
    })}
    {error && <p role="alert">{error}</p>}
  </section>;
}
