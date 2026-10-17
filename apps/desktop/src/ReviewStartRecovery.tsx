import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { CoreClient, TaskId, TurnReviewRun } from "@dogcodes/protocol";
import { reviewStartOperations as operations, type ReviewStartRequest } from "./reviewStartOperations";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { providerLabel } from "./agentPresentation";
import { reviewLabels } from "./ReviewActions";
import { userFacingError } from "./userFacingError";

export function usePendingReview(task: TaskId, scope: string) {
  const saved = useSyncExternalStore(operations.subscribe, () => operations.snapshot(task, scope), () => "null");
  return JSON.parse(saved) as ReviewStartRequest | { error: string } | null;
}
export function ReviewStartRecovery({ client, taskId, scope, busy = false, onRecovered }: {
  client: CoreClient; taskId: TaskId; scope: string; busy?: boolean; onRecovered: (run: TurnReviewRun) => void;
}) {
  const record = usePendingReview(taskId, scope);
  const pending = record && "runId" in record ? record : null;
  const [working, setWorking] = useState(false), [missing, setMissing] = useState(false), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true), context = useRef(""); context.current = `${taskId}:${scope}`;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const id = pending?.runId;
  useEffect(() => { setMissing(false); setError(""); }, [id, taskId, scope]);
  async function recover(resend: boolean) {
    if (!pending || guard.current || busy) return;
    guard.current = true; setWorking(true); setError("");
    const origin = `${taskId}:${scope}`;
    try {
      const run = resend ? await operations.resend(client, scope, pending) : await operations.check(client, scope, pending);
      if (mounted.current && context.current === origin) { setMissing(!run); if (run) onRecovered(run); }
    } catch (cause) { if (mounted.current && context.current === origin) setError(userFacingError(cause, "无法核实审查，请稍后重试。")); }
    finally { guard.current = false; if (mounted.current) setWorking(false); }
  }
  if (!record) return null;
  if ("error" in record) return <InlineError>{record.error}</InlineError>;
  return <div className="turn-action-feedback" role="group" aria-label="待核实的审查请求">
    <p>有一项审查尚未核实：{reviewLabels[record.kind]} · {providerLabel(record.executor.providerId)}。</p>
    <button type="button" disabled={busy || working} onClick={() => void recover(false)}>核实审查结果</button>
    {missing && <><p>后台尚未找到原审查。重发保留原 Agent、范围和标识；原范围不可用时，请核对后结束跟踪再发起新审查。</p><button type="button" disabled={busy || working} onClick={() => void recover(true)}>重发原审查</button></>}
    <button type="button" disabled={busy || working} onClick={() => { try { operations.forget(scope, record); } catch (cause) { setError(String(cause)); } }}>已核对，结束审查跟踪</button>
    <small>结束跟踪不会取消后台审查。</small>
    {working && <LoadingStatus>正在核实审查…</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
