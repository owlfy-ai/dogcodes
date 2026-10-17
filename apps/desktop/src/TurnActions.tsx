import { ReviewStartRecovery, usePendingReview } from "./ReviewStartRecovery";
import { reviewStartOperations } from "./reviewStartOperations";
import { useEnabledAgents } from "./agentPreferences";
import { useContext, useEffect, useRef, useState } from "react";
import { ulid } from "ulid";
import type { CoreClient, ExecutorRef, HistoricalReviewKind, ReviewKind, TaskId, TimelineItem, TurnReviewRequest, TurnReviewRun } from "@dogcodes/protocol";
import { reviewFindingPath } from "./reviewTarget";
import { ReviewResult } from "./ReviewResult";
import { ReviewFailure } from "./reviewFailure";
import { AgentIcon, providerLabel } from "./agentPresentation";
import { reviewAgentExecutors } from "./agentOptions";
import { AgentQuotaContext } from "./AgentQuotaRing";
import { ActionIcon, ReviewActionButton, ReviewAgentPicker, reviewKinds } from "./ReviewActions";
import "./turnActions.css";
import { TurnForkAction } from "./TurnForkAction";
import { TurnStatistics } from "./TurnStatistics";
import { userFacingError } from "./userFacingError";
import { InlineError } from "./InlineError";

const labels: Record<HistoricalReviewKind, string> = { code: "审查代码", tests: "检查测试", requirements: "对照要求", impact: "评估影响", security: "安全检查", performance: "性能分析" };
const supportedKind = (kind: HistoricalReviewKind): kind is ReviewKind => kind === "code" || kind === "security" || kind === "performance";
const states = { queued: "准备中", running: "检查中", cancelling: "正在取消", completed: "已完成", failed: "失败", cancelled: "已取消", interrupted: "连接中断" };
const active = (r: TurnReviewRun) => ["queued", "running", "cancelling"].includes(r.status);
const executorKey = (e: ExecutorRef) => `${e.providerId}:${e.installationId ?? ""}`;
const executorCache = new WeakMap<CoreClient, { at: number; promise: Promise<ExecutorRef[]> }>();
function availableReviewExecutors(client: CoreClient, refresh = false): Promise<ExecutorRef[]> {
  if (!client.reviewExecutors) return Promise.resolve([]);
  const cached = executorCache.get(client);
  if (!refresh && cached && Date.now() - cached.at < 1000) return cached.promise;
  const promise = Promise.all([client.bootstrap(), client.reviewExecutors()])
    .then(([snapshot, connected]) => reviewAgentExecutors(snapshot.installations, connected));
  executorCache.set(client, { at: Date.now(), promise });
  return promise;
}
const cache = new WeakMap<CoreClient, Map<TaskId, { at: number; promise: Promise<TurnReviewRun[]> }>>();
function reviews(client: CoreClient, taskId: TaskId, force = false) {
  let entries = cache.get(client); if (!entries) { entries = new Map(); cache.set(client, entries); }
  const old = entries.get(taskId); if (!force && old && Date.now() - old.at < 1000) return old.promise;
  const promise = client.turnReviews!(taskId); entries.set(taskId, { at: Date.now(), promise });
  return promise;
}
export function TurnActions({ client, items, turnId, completedAt, startedAt, onReviewFile, onForked }: {
  client: CoreClient; items: TimelineItem[]; turnId: string; completedAt: string; startedAt: string; onReviewFile?: (path: string) => void; onForked?: (taskId: TaskId) => Promise<void> | void;
}) {
  const taskId = items[0].taskId;
  const quotaViews = useContext(AgentQuotaContext);
  const [discoveredAgents, setExecutors] = useState<ExecutorRef[]>([]);
  const executors = useEnabledAgents(discoveredAgents);
  const [selected, setSelected] = useState(() => { try { return localStorage.getItem("dogcodes.review.executor") ?? ""; } catch { return ""; } });
  const [runs, setRuns] = useState<TurnReviewRun[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [agentMenu, setAgentMenu] = useState(false);
  const mounted = useRef(true);
  const scope = `turn:${turnId}`;
  const pending = usePendingReview(taskId, scope);
  const freshRuns = useRef(new Set<string>());
  const busyRef = useRef(false);
  useEffect(() => {
    mounted.current = true;
    void availableReviewExecutors(client).then(list => { if (mounted.current) setExecutors(list); }).catch(() => { if (mounted.current) setError("无法读取可用审查 Agent"); });
    return () => { mounted.current = false; };
  }, [client]);
  useEffect(() => {
    if (!agentMenu) return;
    let valid = true;
    void availableReviewExecutors(client, true).then(list => { if (valid) setExecutors(list); }).catch(() => { if (valid) setError("无法读取可用审查 Agent"); });
    return () => { valid = false; };
  }, [agentMenu, client]);
  useEffect(() => {
    if (!client.turnReviews) return;
    let valid = true; let timer: ReturnType<typeof setTimeout>;
    const load = async () => {
      try { const list = (await reviews(client, taskId)).filter(r => r.request.turnId === turnId); if (!valid) return; setRuns(list); timer = setTimeout(load, list.some(active) ? 1500 : 10000); }
      catch { if (valid) { setError("无法读取审查状态"); timer = setTimeout(load, 5000); } }
    };
    void load(); return () => { valid = false; clearTimeout(timer); };
  }, [client, taskId, turnId, busy]);
  const executor = executors.find(e => executorKey(e) === selected) ?? executors.find(e => e.providerId === selected.split(":")[0]) ?? executors[0];
  async function start(kind: ReviewKind, retryOf: string | null = null) {
    if (!executor || !client.startTurnReview || busyRef.current || pending) return;
    busyRef.current = true; setBusy(true); setError("");
    const request: TurnReviewRequest = { taskId, turnId, runId: `run_${ulid()}`, executor, kind, retryOf, recheckOf: runs.find(run => run.request.runId === retryOf)?.request.recheckOf ?? null };
    try { const run = await reviewStartOperations.send(client, scope, request); freshRuns.current.add(run.request.runId); if (mounted.current) setRuns(previous => [...previous.filter(r => r.request.runId !== run.request.runId), run]); await reviews(client, taskId, true); }
    catch (e) { if (mounted.current) setError(userFacingError(e, "发起审查失败，请稍后重试。")); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  async function cancel(run: TurnReviewRun) {
    setError("");
    try { await client.cancelTurnReview?.(taskId, run.request.runId); setRuns((await reviews(client, taskId, true)).filter(r => r.request.turnId === turnId)); }
    catch (e) { setError(userFacingError(e, "取消审查失败，请稍后重试。")); }
  }
  async function copy() {
    const final = [...items].reverse().find(i => i.kind === "assistant_message" || (i.kind === "activity" && (() => { try { return JSON.parse(i.text).type === "message"; } catch { return false; } })()));
    if (!final) return;
    try { await navigator.clipboard.writeText(final.kind === "activity" ? JSON.parse(final.text).text : final.text); setCopied(true); }
    catch { setError("复制失败，请重试"); }
  }
  const disabledReason = !client.startTurnReview ? "请在桌面应用中执行审查" : !executor ? "暂无可用 Agent，请先连接或重新检测 Agent" : "";
  const action = (kind: ReviewKind) => <ReviewActionButton key={kind} kind={kind} tooltip={disabledReason || `${labels[kind]} · ${providerLabel(executor?.providerId ?? "")}`} disabled={busy || !!pending || !!disabledReason || runs.some(r => active(r) && r.request.kind === kind)} onClick={() => void start(kind)}/>;
  return <div className="turn-actions-section">
    <div className="turn-actions" aria-label="本轮操作">
      <div className="turn-actions-left">
        <button type="button" className="turn-action" aria-label="复制本轮回复" data-tooltip={copied ? "已复制" : "复制回复"} onClick={() => void copy()}><ActionIcon name="copy"/></button>
        <TurnStatistics items={items} turnId={turnId} startedAt={startedAt} completedAt={completedAt}/>
        <TurnForkAction client={client} taskId={taskId} turnId={turnId} onForked={onForked}/>
        <time dateTime={completedAt} data-tooltip={`开始：${new Date(startedAt).toLocaleString()}\n结束：${new Date(completedAt).toLocaleString()}\n耗时：${Math.max(0, Math.round((Date.parse(completedAt)-Date.parse(startedAt))/1000))} 秒`}>{new Date(completedAt).toLocaleString(undefined, { month:"numeric",day:"numeric",hour:"2-digit",minute:"2-digit" })}</time>
      </div>
      <div className="turn-actions-right">
        {reviewKinds.map(action)}
        <ReviewAgentPicker executors={executors} executor={executor} quotas={quotaViews} open={agentMenu} onOpenChange={setAgentMenu} onSelect={setSelected}/>
      </div>
    </div>
    {copied && <span role="status" className="turn-action-feedback">已复制</span>}
    <ReviewFailure error={error}/>
    <ReviewStartRecovery client={client} taskId={taskId} scope={scope} busy={busy} onRecovered={run => { freshRuns.current.add(run.request.runId); setRuns(old => [...old.filter(value => value.request.runId !== run.request.runId), run]); cache.get(client)?.delete(taskId); setError(""); }}/>

    {runs.map(run => <details className="turn-review-result" key={run.request.runId} open={freshRuns.current.has(run.request.runId) || active(run) || run.status === "failed" || run.status === "interrupted" ? true : undefined}>
      <summary><AgentIcon providerId={run.request.executor.providerId}/><strong>{labels[run.request.kind]}</strong><span>{providerLabel(run.request.executor.providerId)} · {states[run.status]}</span></summary>
      <ReviewResult client={client} run={run} executor={executor} onReviewFinding={onReviewFile ? finding => onReviewFile(reviewFindingPath(finding)) : undefined}/>
      <footer><small>{new Date(run.createdAt).toLocaleString()} · {run.workspaceReview ? "当前代码" : "历史审查记录"}</small>{active(run) ? <button type="button" disabled={run.status === "cancelling"} onClick={() => void cancel(run)}>取消审查</button> : supportedKind(run.request.kind) ? <button type="button" disabled={busy || !!pending || !!disabledReason} data-tooltip="根据本轮线索重新检查当前代码" onClick={() => { if (supportedKind(run.request.kind)) void start(run.request.kind,run.workspaceReview ? run.request.runId : null); }}>重新检查</button> : null}</footer>
    </details>)}
  </div>;
}
