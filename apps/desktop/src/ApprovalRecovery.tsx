import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { CoreClient, TaskId } from "@dogcodes/protocol";
import { approvalOperations as operations, type ApprovalCheck, type PendingApprovalResponse } from "./approvalOperations";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { userFacingError } from "./userFacingError";

export function usePendingApprovalResponses(taskId: TaskId | null) {
  const saved = useSyncExternalStore(operations.subscribe, () => operations.snapshot(taskId), () => "[]");
  return JSON.parse(saved) as PendingApprovalResponse[] | { error: string };
}
function RecoveryItem({ client, record, busy, revision, onSettled }: { client: CoreClient; record: PendingApprovalResponse; busy: boolean; revision: number; onSettled: (result: ApprovalCheck) => void }) {
  const [working, setWorking] = useState(false), [result, setResult] = useState<ApprovalCheck | null>(null), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true), refreshAgain = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function check(changeDecision = false, automatic = false) {
    if (guard.current || busy) { if (automatic && !busy) refreshAgain.current = true; return; }
    guard.current = true; setWorking(true); setError("");
    try {
      const value = changeDecision ? await operations.changeDecision(client, record) : await operations.check(client, record);
      onSettled(value); // Matched success can remove this row before resolving.
      if (mounted.current) setResult(value);
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, "暂时无法核实审批结果，请稍后重试。")); }
    finally {
      guard.current = false;
      if (mounted.current) {
        setWorking(false);
        if (refreshAgain.current) { refreshAgain.current = false; void check(false, true); }
      }
    }
  }
  // Reuse the task projection's revision, not a second event subscription. A
  // delivery event racing an older lookup gets one trailing read-only refresh.
  useEffect(() => { if (!busy) void check(false, true); }, [client, record.query.operationId, busy, revision]);
  const accepted = result?.kind === "accepted" || record.accepted;
  const failed = result?.kind === "accepted" && result.receipt.delivery === "failed";
  return <div className="turn-action-feedback" role="group" aria-label="审批结果待核实">
    <p>{new Date(record.requestedAt).toLocaleString()} 提交的审批或回答</p>
    <p>{failed ? "决定已受理，驱动报告投递失败。请查看会话状态；不会重复提交这个决定。"
      : accepted ? "决定已受理，投递结果尚未确认；已受理不代表任务已完成。"
      : result?.kind === "missing" ? "尚未找到原回执，不代表请求已失败。若原请求仍在，请在审批卡片中重新选择或输入原回答；提交时会先核实，并只重发相同的原请求。"
      : "正在核实原请求。重开窗口不会自动重新提交审批。"}</p>
    <button type="button" disabled={busy || working} onClick={() => void check()}>核实审批结果</button>
    {!accepted && result?.kind === "missing" && <button type="button" disabled={busy || working} onClick={() => void check(true)}>修改未受理的决定</button>}
    {record.accepted && <button type="button" disabled={busy || working} onClick={() => { try { operations.dismiss(record); } catch (cause) { setError(String(cause)); } }}>已核对，结束审批跟踪</button>}
    <small>恢复记录不保存回答正文。修改决定会先让原版本失效；已受理的决定不能撤回。</small>
    {working && <LoadingStatus>正在核实审批结果…</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
export function ApprovalRecovery({ client, taskId, busy, revision = 0, onAccepted }: { client: CoreClient; taskId: TaskId; busy: boolean; revision?: number; onAccepted: (record: PendingApprovalResponse) => void }) {
  const pending = usePendingApprovalResponses(taskId);
  const [notice, setNotice] = useState("");
  const mounted = useRef(true), scope = useRef({ client, taskId, revision: 0 });
  if (scope.current.client !== client || scope.current.taskId !== taskId) scope.current = { client, taskId, revision: scope.current.revision + 1 };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => setNotice(""), [client, taskId]);
  const origin = scope.current;
  const settled = (record: PendingApprovalResponse, result: ApprovalCheck) => {
    if (!mounted.current || scope.current !== origin || result.kind === "missing") return;
    if (result.kind === "obsolete") setNotice("原审批未被受理，会话版本已更新。请重新查看请求，再确认提交。");
    else setNotice("");
    onAccepted(record);
  };
  if (!Array.isArray(pending)) return <InlineError>{pending.error}</InlineError>;
  return <>{notice && <p role="status" className="turn-action-feedback">{notice}</p>}
    {pending.map(record => <RecoveryItem key={`${origin.revision}:${record.query.operationId}`} client={client} record={record} busy={busy} revision={revision} onSettled={result => settled(record, result)}/>)}</>;
}
