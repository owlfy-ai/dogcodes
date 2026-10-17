import { ReviewStartRecovery, usePendingReview } from "./ReviewStartRecovery";
import { reviewStartOperations } from "./reviewStartOperations";
import { ReviewRepairRecovery, usePendingRepair } from "./ReviewRepairRecovery";
import { reviewRepairOperations } from "./reviewRepairOperations";
import { useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { ulid } from "ulid";
import type { BootstrapSnapshot, CoreClient, ExecutorRef, ReviewKind, TurnReviewRequest, TurnReviewRun } from "@dogcodes/protocol";
import { providerLabel } from "./agentPresentation";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { PendingExecutorContext } from "./automaticContext";

const active = (run: TurnReviewRun) => ["queued", "running", "cancelling"].includes(run.status);
const status: Record<string, string> = {running: "修复中", completed: "修复轮次已完成", failed: "修复失败", cancelled: "修复已取消", interrupted: "修复已中断"};
// Several reports may be mounted in the turn and Diff panels at once.
const cache = new WeakMap<CoreClient, Map<string, {at: number; value: Promise<[TurnReviewRun[], BootstrapSnapshot]>}>>();
function progress(client: CoreClient, taskId: TurnReviewRun["request"]["taskId"]) {
  let entries = cache.get(client); if (!entries) {entries = new Map(); cache.set(client, entries);}
  const old = entries.get(taskId); if (old && Date.now() - old.at < 1000) return old.value;
  const value = Promise.all([client.turnReviews?.(taskId) ?? [], client.bootstrap()]);
  entries.set(taskId, {at: Date.now(), value}); return value;
}
export function ReviewRepairControls({client, run, executor, selected, clearSelection, renderReport}: {
  client: CoreClient; run: TurnReviewRun; executor?: ExecutorRef; selected: number[]; clearSelection: () => void; renderReport: (run: TurnReviewRun) => ReactNode;
}) {
  const [all, setAll] = useState<TurnReviewRun[]>([]);
  const [primary, setPrimary] = useState<{busy: boolean} | null>(null);
  const pendingExecutor = useContext(PendingExecutorContext)[run.request.taskId];
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const guard = useRef(false);
  const scopeKey = `${run.request.taskId}:${run.request.runId}`;
  const mounted = useRef(true), scope = useRef({ key: scopeKey, client });
  if (scope.current.key !== scopeKey || scope.current.client !== client) scope.current = { key: scopeKey, client };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const repairState = usePendingRepair(client, run.request.taskId, run.request.runId);
  const pendingRepair = repairState.entry;
  const recheckScope = `recheck:${run.request.runId}`;
  const pendingReview = usePendingReview(run.request.taskId, recheckScope);
  useEffect(() => {
    let valid = true; let timer: ReturnType<typeof setTimeout>;
    const load = async () => {
      try {
        const [runs, bootstrap] = await progress(client, run.request.taskId);
        if (!valid) return;
        setAll(runs);
        const task = bootstrap.tasks.find(t => t.taskId === run.request.taskId);
        setPrimary(task ? {busy: !!task.activePrimaryRun} : null);
      } catch { if (valid) setError("无法读取修复进度，请稍后重试"); }
      if (valid) timer = setTimeout(load, 3000);
    };
    void load(); return () => {valid = false; clearTimeout(timer);};
  }, [client, run.request.taskId, revision]);
  const current = all.find(r => r.request.runId === run.request.runId) ?? run;
  function repaired() {
    clearSelection(); cache.get(client)?.delete(run.request.taskId); setRevision(value => value + 1); setError("");
  }
  async function repair() {
    if (guard.current || pendingRepair || !selected.length || !client.reviewRepair) return;
    guard.current = true; setBusy(true); setError("");
    const origin = scope.current;
    try {
      await reviewRepairOperations.start(client, run.request.taskId, {runId: run.request.runId, inputSha256: run.inputSha256, findingIndices: [...selected]}, pendingExecutor);
      if (mounted.current && scope.current === origin) repaired();
    } catch (cause) { if (mounted.current && scope.current === origin) setError(userFacingError(cause, "修复结果尚未确认，请核实原操作。")); }
    finally {guard.current = false; if (mounted.current) setBusy(false);}
  }
  async function recheck(turnId: string) {
    if (guard.current || pendingReview || !executor || !client.startTurnReview || !["code", "security", "performance"].includes(run.request.kind)) return;
    guard.current = true; setBusy(true); setError("");
    const origin = scope.current;
    const request: TurnReviewRequest = {taskId: run.request.taskId, turnId, runId: `run_${ulid()}`, executor, kind: run.request.kind as ReviewKind, retryOf: null, recheckOf: {runId: run.request.runId, repairTurnId: turnId}};
    try {await reviewStartOperations.send(client, recheckScope, request); cache.get(client)?.delete(run.request.taskId); if (mounted.current && scope.current === origin) setRevision(value => value + 1);}
    catch (cause) {if (mounted.current && scope.current === origin) setError(userFacingError(cause, "发起复审失败，请稍后重试。"));}
    finally {guard.current = false; if (mounted.current) setBusy(false);}
  }
  const hint = pendingRepair ? "原修复结果待核实" : primary?.busy ? "主对话正在回复，结束后可提交修复" : selected.length ? `已选 ${selected.length} 项` : "勾选问题后交给主 Agent 修复";
  return <div className="review-repairs">
    <div className="review-repair-bar">
      <span className="review-repair-hint">{hint}</span>
      <button type="button" className="review-repair-submit" disabled={!!pendingRepair || !selected.length || !primary || primary.busy || busy} onClick={() => void repair()}>开始修复</button>
    </div>
    <ReviewRepairRecovery client={client} pending={pendingRepair} busy={busy} onRecovered={repaired}/>
    {repairState.error && <InlineError>{repairState.error}</InlineError>}
    {current.repairs?.map(repair => {
      const children = all.filter(r => repair.recheckRuns.includes(r.request.runId));
      return <div className="review-repair" key={repair.turnId}>
        <p>{repair.findingIndices.map(i => `问题 ${i + 1}`).join("、")} · {status[repair.status] ?? repair.status}{children.length ? " · 已发起复审" : ""}</p>
        {repair.status === "completed" && <button type="button" disabled={busy || !!pendingReview || !executor || children.some(active)} onClick={() => void recheck(repair.turnId)}>复审修复后的版本 · {providerLabel(executor?.providerId ?? "")}</button>}
        {children.map(child => <details key={child.request.runId}><summary>修复后复审 · {providerLabel(child.request.executor.providerId)} · {active(child) ? "进行中" : child.status === "completed" ? "已完成" : "未完成"}</summary>
          <p className="turn-review-notice">{child.workspaceReview ? "当前代码复审" : "历史版本复审"} · {new Date(child.createdAt).toLocaleString()}</p>{renderReport(child)}
          {active(child) && <button type="button" disabled={child.status === "cancelling"} onClick={() => void client.cancelTurnReview?.(child.request.taskId, child.request.runId).then(() => setRevision(value => value + 1)).catch(cause => setError(userFacingError(cause, "取消复审失败，请稍后重试。")))}>取消复审</button>}
        </details>)}
      </div>;
    })}
    <ReviewStartRecovery client={client} taskId={run.request.taskId} scope={recheckScope} busy={busy} onRecovered={() => { cache.get(client)?.delete(run.request.taskId); setRevision(value => value + 1); setError(""); }}/>
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
