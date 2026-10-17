import { useEffect, useRef, useState } from "react";
import type { CoreClient, ReviewRepairEntry, TaskId } from "@dogcodes/protocol";
import { reviewRepairOperations as operations } from "./reviewRepairOperations";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { providerLabel } from "./agentPresentation";
import { userFacingError } from "./userFacingError";

const REFRESH_MS = 3000;

/** The service record of an unfinished repair for this review run, shared by all clients. */
export function usePendingRepair(client: CoreClient, taskId: TaskId, runId: string) {
  const [state, setState] = useState<{ key: string; entry: ReviewRepairEntry | null; error: string }>({ key: "", entry: null, error: "" });
  const scopeKey = `${taskId}:${runId}`;
  useEffect(() => {
    if (!client.reviewRepair) return;
    let valid = true, timer: ReturnType<typeof setTimeout> | undefined, generation = 0;
    const load = async () => {
      clearTimeout(timer);
      const current = ++generation;
      try {
        const entry = await operations.get(client, taskId, runId);
        if (valid && current === generation) setState({ key: scopeKey, entry: entry?.state === "pending" ? entry : null, error: "" });
      } catch (cause) {
        if (valid && current === generation) setState(previous => ({ ...previous, key: scopeKey, error: userFacingError(cause, "无法读取修复状态，请稍后重试。") }));
      }
      if (valid && current === generation) timer = setTimeout(() => void load(), REFRESH_MS);
    };
    void load();
    const unsubscribe = operations.subscribe(() => void load());
    return () => { valid = false; clearTimeout(timer); unsubscribe(); };
  }, [client, taskId, runId, scopeKey]);
  return state.key === scopeKey ? state : { key: scopeKey, entry: null, error: "" };
}

export function ReviewRepairRecovery({ client, pending, busy, onRecovered }: {
  client: CoreClient; pending: ReviewRepairEntry | null; busy: boolean; onRecovered: () => void;
}) {
  const [working, setWorking] = useState(false), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true);
  const operationId = pending?.intent.operationId;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setError(""); }, [operationId]);
  async function act(action: "resume" | "abandon") {
    if (!pending || busy || guard.current) return;
    guard.current = true; setWorking(true); setError("");
    try {
      const latest = await operations[action](client, pending);
      // Never let a late reply clear a newer request's selection.
      if (mounted.current && latest.intent.operationId === operationId && latest.state === "submitted") onRecovered();
    } catch (cause) {
      if (mounted.current) setError(userFacingError(cause, action === "resume" ? "无法继续修复，请稍后重试。" : "无法放弃修复，请稍后重试。"));
    } finally { guard.current = false; if (mounted.current) setWorking(false); }
  }
  if (!pending || pending.state !== "pending") return null;
  return <div className="turn-action-feedback" role="group" aria-label="待核实的修复请求">
    <p>有一项修复尚未提交：{pending.intent.reference.findingIndices.map(index => `问题 ${index + 1}`).join("、")} · {providerLabel(pending.intent.target.providerId)}。</p>
    {pending.contextConfirmed && <p>原上下文已确认，Agent 已切换。</p>}
    {pending.lastError && <p>上次停止原因：{pending.lastError}</p>}
    <button type="button" disabled={busy || working} onClick={() => void act("resume")}>继续原修复</button>
    <button type="button" disabled={busy || working} onClick={() => void act("abandon")}>放弃这次修复</button>
    <small>继续时保留原问题、Agent 和版本条件，会话已变化时停止提交。放弃后原请求不会再提交，已完成的 Agent 切换不会撤销。</small>
    {working && <LoadingStatus>正在处理修复…</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
