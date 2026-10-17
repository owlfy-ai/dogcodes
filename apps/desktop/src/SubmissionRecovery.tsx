import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { CoreClient } from "@dogcodes/protocol";
import { submissionOperations as operations, type PendingSubmission, type SubmissionResult } from "./submissionOperations";
import { LoadingStatus } from "./LoadingStatus";
import { InlineError } from "./InlineError";
import { providerLabel } from "./agentPresentation";
import { userFacingError } from "./userFacingError";

export function usePendingSubmissions(client: CoreClient) {
  useEffect(() => operations.watch(client), [client]);
  const snapshot = useCallback(() => operations.snapshot(client), [client]);
  return JSON.parse(useSyncExternalStore(operations.subscribe, snapshot, () => "[]")) as PendingSubmission[] | { error: string; records: PendingSubmission[] };
}
function RecoveryItem({ client, record, busy, canResume, onResolved }: {
  client: CoreClient; record: PendingSubmission; busy: boolean; canResume: boolean;
  onResolved: (record: PendingSubmission, result: SubmissionResult) => void;
}) {
  const [working, setWorking] = useState(false), [checked, setChecked] = useState(false), [error, setError] = useState("");
  const guard = useRef(false), mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function recover(action: "check" | "resume" | "retire") {
    if (busy || guard.current || (action === "resume" && !canResume)) return;
    guard.current = true; setWorking(true); setError("");
    try {
      const result = await operations[action](client, record);
      if (result.status === "submitted" || result.status === "invalidated" || result.status === "task_deleted") onResolved(record, result);
      else if (mounted.current) setChecked(true);
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, "暂时无法核实发送结果，请稍后重试。")); }
    finally { guard.current = false; if (mounted.current) setWorking(false); }
  }
  return <div className="turn-action-feedback" role="group" aria-label="待核实的发送请求">
    <p>{record.task.title} · {providerLabel(record.target.providerId)} · {new Date(record.requestedAt).toLocaleString()}</p>
    <p>发送尚未完成，原消息和发送选项已保存在后台，其他窗口或设备也能看到。</p>
    <details><summary>查看原发送内容</summary><p className="submission-original-prompt">{record.input.prompt}</p>
      {!!record.input.attachments.length && <p>附件：{record.input.attachments.map(item => item.name).join("、")}</p>}
      <small>模型：{record.input.model ?? "默认"} · 思考强度：{record.input.thinking ?? "默认"} · 权限：{record.input.permissionMode ?? "默认"}</small>
    </details>
    <button type="button" disabled={busy || working} onClick={() => void recover("check")}>核实发送结果</button>
    {checked && <><p>原消息尚未受理。继续会使用原消息、Agent 和版本；会话已变化时不会自动改用新版本。</p>
      <button type="button" disabled={busy || working || !canResume} onClick={() => void recover("resume")}>继续原发送</button>
      {!canResume && <small>请先恢复原 Agent 的可用状态，再继续发送。</small>}
    </>}
    <button type="button" disabled={busy || working} onClick={() => void recover("retire")}>放弃原发送</button>
    <small>放弃会阻止尚未受理的原请求。已创建的会话或已确认的 Agent 切换会保留；消息若已受理，将显示受理结果。</small>
    {working && <LoadingStatus>正在核实原发送…</LoadingStatus>}
    {error && <InlineError>{error}</InlineError>}
  </div>;
}
export function SubmissionRecovery({ client, busy, canResume, onResolved }: {
  client: CoreClient; busy: boolean; canResume: (record: PendingSubmission) => boolean;
  onResolved: (record: PendingSubmission, result: SubmissionResult) => void;
}) {
  const records = usePendingSubmissions(client);
  const [notice, setNotice] = useState("");
  const mounted = useRef(true), activeClient = useRef(client);
  activeClient.current = client;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  // A normally progressing first attempt is already represented by the composer.
  // Show recovery only after it settles, not a warning during every healthy send.
  if (busy) return null;
  const readable = Array.isArray(records) ? records : records.records;
  return <>{!Array.isArray(records) && <div><InlineError>{records.error}</InlineError>
    {client.submissionIntentList && <button type="button" onClick={() => void operations.discover(client)}>重新核实后台记录</button>}</div>}
    {readable.map(record => <RecoveryItem key={record.operationId} client={client} record={record} busy={busy} canResume={canResume(record)} onResolved={(record, result) => {
    if (!mounted.current || activeClient.current !== client) return;
    setNotice(result.status === "submitted" ? "原消息已受理，请在对应会话查看执行状态。"
      : result.status === "task_deleted" ? "原会话已永久删除，发送恢复记录已结束。" : "已放弃尚未受理的原发送。");
    onResolved(record, result);
  }}/>)}{notice && <p role="status">{notice} <button type="button" onClick={() => setNotice("")}>关闭提示</button></p>}</>;
}
