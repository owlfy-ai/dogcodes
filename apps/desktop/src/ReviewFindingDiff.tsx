import { LoadingStatus } from "./LoadingStatus";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { CoreClient, ReviewFinding, ReviewSource, TurnReviewRun } from "@dogcodes/protocol";
import { focusDialogTitle } from "./dialogFocus";
import { ReviewText } from "./ReviewText";
import { sourceDiff } from "./reviewSourceDiff";
import "./turnActions.css";

export function ReviewFindingDiff({ client, run, finding, onClose }: {
  client: CoreClient; run: TurnReviewRun; finding: ReviewFinding; onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null), titleId = useId();
  const [source, setSource] = useState<ReviewSource | null>(null), [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    const trigger = document.activeElement;
    dialog.current?.showModal(); focusDialogTitle(dialog.current);
    return () => { if (trigger instanceof HTMLElement && trigger.isConnected) trigger.focus(); };
  }, []);
  useEffect(() => {
    let valid = true; setSource(null); setError("");
    if (!client.reviewSource) { setError("当前版本无法读取审查材料，请更新桌面服务。"); return; }
    void client.reviewSource(run.request.taskId, run.request.runId, finding.fileId).then(value => {
      if (!valid) return;
      if (value.fileId !== finding.fileId || value.rootId !== finding.rootId || value.path !== finding.path) { setError("历史文件来源不匹配，未展示其他版本。"); return; }
      setSource(value);
    }, () => { if (valid) setError("审查材料读取或校验失败，未改用当前文件。"); });
    return () => { valid = false; };
  }, [client, run.request.taskId, run.request.runId, finding, retry]);
  const diff = useMemo(() => source ? sourceDiff(source) : null, [source]);
  const excerpt = source?.[finding.side]?.split(/\r?\n/).slice(Math.max(0, finding.startLine - 6), finding.endLine + 5);
  return <dialog ref={dialog} className="historical-turn-diff review-finding-diff" aria-labelledby={titleId} onCancel={onClose}>
    <header><h2 tabIndex={-1} id={titleId}>审查时的历史 Diff</h2><button type="button" aria-label="关闭审查 Diff" onClick={onClose}>×</button></header>
    <p>{finding.path} · {finding.side === "before" ? "改动前" : "改动后"} {finding.startLine}–{finding.endLine} 行</p>
    <p>{new Date(run.createdAt).toLocaleString()}</p>
    {error ? <p role="alert">{error}<button type="button" onClick={() => setRetry(n => n + 1)}>重试</button></p> : !source ? <LoadingStatus layout="panel">正在读取审查材料…</LoadingStatus> : diff ?
      <ReviewText diff={diff} split={false} path={source.path} focusLine={{ side: finding.side, start: finding.startLine, end: finding.endLine }}/> :
      <><p role="status">原文不完整或文件超出差异展示预算，以下只展示已保存的定位片段。</p><pre>{excerpt?.map((line, index) => `${Math.max(1, finding.startLine - 5) + index}  ${line}`).join("\n") ?? "引用侧原文不可用"}</pre></>}
  </dialog>;
}
