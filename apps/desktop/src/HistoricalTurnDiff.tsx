import { LoadingStatus } from "./LoadingStatus";
import { useEffect, useId, useRef, useState } from "react";
import type { ArtifactMetadata, CoreClient, TaskId } from "@dogcodes/protocol";
import { loadTurnArtifact, type TurnArtifact } from "./turnArtifactCache";
import { StructuredDiffView } from "./StructuredDiffView";
import { focusDialogTitle } from "./dialogFocus";

export function HistoricalTurnDiff({ client, taskId, metadata, initialPath, onClose }: { client: CoreClient; taskId: TaskId; metadata: ArtifactMetadata; initialPath?: string; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const [artifact, setArtifact] = useState<TurnArtifact | null>(null);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  useEffect(() => { dialog.current?.showModal(); focusDialogTitle(dialog.current); }, []);
  useEffect(() => {
    let valid = true; setArtifact(null); setError("");
    void loadTurnArtifact(client, taskId, metadata).then(a => { if (valid) setArtifact(a); }, () => { if (valid) setError("历史快照读取或校验失败，未改用当前文件。"); });
    return () => { valid = false; };
  }, [client, taskId, metadata.artifactId, metadata.payloadSha256, retry]);
  const rendered = new Set(artifact?.diff?.files.flatMap(f => [f.oldPath, f.newPath]) ?? []);
  const missing = artifact?.data.files.filter(f => !rendered.has(f.path)) ?? [];
  return <dialog ref={dialog} className="historical-turn-diff" onCancel={onClose} aria-labelledby={titleId}>
    <header><h2 id={titleId} tabIndex={-1}>本轮历史 Diff</h2><button type="button" onClick={onClose} aria-label="关闭历史 Diff">×</button></header>
    <p>固定于 {new Date(metadata.createdAt).toLocaleString()} · 只读历史记录</p>
    <p className="turn-review-notice">这是本轮期间采集的工作区改动，可能包含其他 Agent 或用户的修改；之后的文件修改不会改变此视图。</p>
    {error ? <p role="alert">{error} <button type="button" onClick={() => setRetry(n => n + 1)}>重试</button></p> : !artifact ? <LoadingStatus layout="panel">正在读取历史快照…</LoadingStatus> : <>
      {artifact.data.attribution === "workspace_recovery_interval" && <p role="status">这份记录包含服务中断至恢复期间的变化。</p>}
      {artifact.diff && <StructuredDiffView diff={artifact.diff} artifactKey={metadata.artifactId} initialPath={initialPath} />}
      {missing.length > 0 && <div className="turn-review-notice"><p>以下文件缺少可展示的文本差异（原文未保存、二进制或解析范围受限）：</p><ul>{missing.map(f => <li key={f.path}>{f.path}</li>)}</ul></div>}
      {!artifact.data.files.length && <p>该轮保存的快照没有文件变化。</p>}
      {artifact.data.patch && <details><summary>查看原始历史补丁</summary><pre>{artifact.data.patch}</pre></details>}
    </>}
  </dialog>;
}
