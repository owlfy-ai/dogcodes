import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { CoreClient, TaskId } from "@dogcodes/protocol";
import { cancelOperations as operations, type PendingCancellation } from "./cancelOperations";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { userFacingError } from "./userFacingError";

export function usePendingCancellations(taskId: TaskId | null) {
  const saved = useSyncExternalStore(operations.subscribe, () => operations.snapshot(taskId), () => "[]");
  return JSON.parse(saved) as PendingCancellation[] | { error: string };
}
function CancelRecoveryItem({ client, record, busy, onAccepted }: { client: CoreClient; record: PendingCancellation; busy: boolean; onAccepted: () => void }) {
  const [working, setWorking] = useState(false), [missing, setMissing] = useState(false), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function recover(resend: boolean) {
    if (guard.current || busy) return;
    guard.current = true; setWorking(true); setError("");
    try {
      const accepted = resend ? (await operations.resend(client, record), true) : await operations.check(client, record);
      // A matched receipt removes this row before its promise can resolve. The
      // parent guards task lifetime so a still-visible task can refresh anyway.
      if (accepted) onAccepted();
      if (mounted.current) setMissing(!accepted);
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, "无法核实停止请求，请稍后重试。")); }
    finally { guard.current = false; if (mounted.current) setWorking(false); }
  }
  return <div className="turn-action-feedback" role="group" aria-label="待核实的停止请求">
    <p>{new Date(record.requestedAt).toLocaleString()} 发起的停止请求尚未核实。</p>
    <button type="button" disabled={busy || working} onClick={() => void recover(false)}>核实停止请求</button>
    {missing && <><p>尚未找到原回执，不代表请求已失败。重发只针对原执行轮次；目标或版本已变化时会拒绝，不会停止新的任务。</p>
      <button type="button" disabled={busy || working} onClick={() => void recover(true)}>重发原停止请求</button></>}
    <button type="button" disabled={busy || working} onClick={() => { try { operations.forget(record); } catch (cause) { setError(String(cause)); } }}>已核对，结束停止跟踪</button>
    <small>受理停止请求后仍需等待任务实际退出；结束跟踪不会撤回已发送的停止请求。</small>
    {working && <LoadingStatus>正在核实停止请求…</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
export function CancelRecovery({ client, taskId, busy, onAccepted }: { client: CoreClient; taskId: TaskId; busy: boolean; onAccepted: () => void }) {
  const pending = usePendingCancellations(taskId);
  const mounted = useRef(true), scope = useRef({ client, taskId });
  if (scope.current.client !== client || scope.current.taskId !== taskId) scope.current = { client, taskId };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const origin = scope.current;
  const accepted = () => { if (mounted.current && scope.current === origin) onAccepted(); };
  if (!Array.isArray(pending)) return <InlineError>{pending.error}</InlineError>;
  return <>{pending.map(record => <CancelRecoveryItem key={record.operationId} client={client} record={record} busy={busy} onAccepted={accepted}/>)}</>;
}
