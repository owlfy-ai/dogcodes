import { LoadingStatus } from "./LoadingStatus";
import { useEffect, useId, useRef, useState } from "react";
import type { ArtifactMetadata, CoreClient, TaskId } from "@dogcodes/protocol";
import { loadTurnArtifact, type TurnArtifact } from "./turnArtifactCache";
import { StructuredDiffView } from "./StructuredDiffView";
import { focusDialogTitle } from "./dialogFocus";
import { translate as t } from "./i18n";

export function HistoricalTurnDiff({ client, taskId, metadata, initialPath, onClose }: { client: CoreClient; taskId: TaskId; metadata: ArtifactMetadata; initialPath?: string; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const [artifact, setArtifact] = useState<TurnArtifact | null>(null);
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  useEffect(() => { dialog.current?.showModal(); focusDialogTitle(dialog.current); }, []);
  useEffect(() => {
    let valid = true; setArtifact(null); setError("");
    void loadTurnArtifact(client, taskId, metadata).then(a => { if (valid) setArtifact(a); }, () => { if (valid) setError(t("historicalDiff.loadFailed")); });
    return () => { valid = false; };
  }, [client, taskId, metadata.artifactId, metadata.payloadSha256, retry]);
  const rendered = new Set(artifact?.diff?.files.flatMap(f => [f.oldPath, f.newPath]) ?? []);
  const missing = artifact?.data.files.filter(f => !rendered.has(f.path)) ?? [];
  return <dialog ref={dialog} className="historical-turn-diff" onCancel={onClose} aria-labelledby={titleId}>
    <header><h2 id={titleId} tabIndex={-1}>{t("historicalDiff.title")}</h2><button type="button" onClick={onClose} aria-label={t("historicalDiff.close")}>×</button></header>
    <p>{t("historicalDiff.pinned", { time: new Date(metadata.createdAt).toLocaleString() })}</p>
    <p className="turn-review-notice">{t("historicalDiff.notice")}</p>
    {error ? <p role="alert">{error} <button type="button" onClick={() => setRetry(n => n + 1)}>{t("common.retry")}</button></p> : !artifact ? <LoadingStatus layout="panel">{t("historicalDiff.loading")}</LoadingStatus> : <>
      {artifact.data.attribution === "workspace_recovery_interval" && <p role="status">{t("historicalDiff.recoveryInterval")}</p>}
      {artifact.diff && <StructuredDiffView diff={artifact.diff} artifactKey={metadata.artifactId} initialPath={initialPath} />}
      {missing.length > 0 && <div className="turn-review-notice"><p>{t("historicalDiff.missing")}</p><ul>{missing.map(f => <li key={f.path}>{f.path}</li>)}</ul></div>}
      {!artifact.data.files.length && <p>{t("historicalDiff.empty")}</p>}
      {artifact.data.patch && <details><summary>{t("historicalDiff.rawPatch")}</summary><pre>{artifact.data.patch}</pre></details>}
    </>}
  </dialog>;
}
