import { HistoricalTurnDiff } from "./HistoricalTurnDiff";
import { TurnUndoAction } from "./TurnUndoAction";
import { InlineFileEdit } from "./InlineFileEdit";
import { cachedTurnArtifact, loadTurnArtifact, turnArtifactKey, type TurnArtifact } from "./turnArtifactCache";
import { useEffect, useState } from "react";
import { OpenFileButton } from "./OpenFileButton";
import type {
  ArtifactMetadata,
  CoreClient,
  TaskId,
} from "@dogcodes/protocol";
import "./turnSummary.css";

export function TurnFiles({
  taskId,
  metadata,
  client,
  operation = false,
}: {
  taskId: TaskId;
  metadata: ArtifactMetadata;
  client: CoreClient;
  operation?: boolean;
  onReviewFile?: (path: string) => void;
}) {
  const key = turnArtifactKey(taskId, metadata);
  const [loaded, setLoaded] = useState<{ key: string; artifact: TurnArtifact } | null>(null);
  const artifact = cachedTurnArtifact(client, taskId, metadata) ?? (loaded?.key === key ? loaded.artifact : null);
  const data = artifact?.data ?? null, diff = artifact?.diff ?? null;
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  const [historicalOpen, setHistoricalOpen] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setError("");
    if (!cachedTurnArtifact(client, taskId, metadata)) {
      void loadTurnArtifact(client, taskId, metadata).then(value => {
        if (active) setLoaded({ key, artifact: value });
      }).catch(() => { if (active) setError("本轮变更加载失败"); });
    }
    return () => { active = false; };
  }, [client, taskId, key, retry]);
  if (error && !data)
    return (
      <p role="alert">
        {error}{" "}
        <button type="button" onClick={() => setRetry((n) => n + 1)}>
          重试
        </button>
      </p>
    );
  // Historical artifacts are already captured. Loading them must not insert a
  // temporary row that disappears when the saved change list is empty.
  if (!data) return null;
  if (!data.files.length) return null;
  if (operation) return <>
    {(data.attribution === "workspace_recovery_interval" || data.recoveredAfterInterruption) && <p className="subagent-notice">连接中断后的恢复快照 · 仅反映检查时的文件变化，外部工具可能仍在写入</p>}
    {data.files.map(f => <InlineFileEdit key={f.path} path={f.path}
      file={diff?.files.find(d => (d.newPath || d.oldPath) === f.path)} truncated={diff?.truncated}
      openControl={f.status === "D" ? undefined : <OpenFileButton taskId={taskId} path={f.path} />} />)}
    {error && <p role="alert">{error}</p>}
  </>;
  return (
    <>
    {historicalOpen !== null && <HistoricalTurnDiff client={client} taskId={taskId} metadata={metadata} initialPath={historicalOpen} onClose={() => setHistoricalOpen(null)}/>}
    <details className="turn-files turn-summary" open={artifact?.expanded ?? false} onToggle={event => { if (artifact) artifact.expanded = event.currentTarget.open; }}>
      <summary className="turn-summary-heading">
        <svg className="turn-summary-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m6 4 4 4-4 4" /></svg>
        <strong>{data.files.length} 个文件已更改</strong>
        {diff && <span className="turn-summary-counts"><span className="additions">+{diff.additions}</span><span className="deletions">−{diff.deletions}</span></span>}
        <TurnUndoAction key={key} taskId={taskId} metadata={metadata} count={data.files.length} />
      </summary>
      <div className="turn-summary-list">
        {(data.attribution === "workspace_recovery_interval" || data.recoveredAfterInterruption) && <p className="turn-summary-notice">连接中断后的恢复快照 · 仅反映检查时的文件变化，外部工具可能仍在写入</p>}
        {data.files.map(f => {
          const change = diff?.files.find(d => (d.newPath || d.oldPath) === f.path);
          const separator = Math.max(f.path.lastIndexOf("/"), f.path.lastIndexOf("\\"));
          const name = f.path.slice(separator + 1), directory = f.path.slice(0, separator + 1);
          const extension = name.split(".").pop()?.toLowerCase() ?? "";
          const language = ["ts", "tsx"].includes(extension) ? "TS" : ["js", "jsx", "cjs", "mjs"].includes(extension) ? "JS" : extension.slice(0, 3).toUpperCase() || "◇";
          return <div className="turn-summary-file" key={f.path}>
            <span className={`turn-summary-icon file-${language.toLowerCase()}`} aria-hidden="true">{language}</span>
            <span className="turn-summary-name" data-tooltip={f.path}>{name}</span>
            <span className="turn-summary-directory" data-tooltip={directory}>{directory}</span>
            {change && !change.binary && <span className="turn-summary-counts"><span className="additions">+{change.additions}</span><span className="deletions">−{change.deletions}</span></span>}
            <span className="turn-summary-actions">
              <button type="button" data-tooltip="查看这一轮保存的历史 Diff" aria-label={`查看 ${name} 的历史 Diff`} onClick={() => setHistoricalOpen(f.path)}>历史 Diff</button>
              <OpenFileButton taskId={taskId} path={f.path} disabled={f.status === "D"} />
            </span>
          </div>;
        })}
        {diff?.truncated && <p className="turn-summary-notice">变更统计仅包含已解析的部分差异。</p>}
        {error && <p className="turn-summary-notice" role="alert">{error}</p>}
      </div>
    </details>
    </>
  );
}
