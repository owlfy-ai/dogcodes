import { LoadingStatus } from "./LoadingStatus";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { ArtifactMetadata, CoreClient, TaskId } from "@dogcodes/protocol";
import { loadTurnArtifact, loadTurnFile, type TurnArtifact } from "./turnArtifactCache";
import { HistoricalDiffView } from "./HistoricalDiffView";
import { focusDialogTitle } from "./dialogFocus";
import { translate as t } from "./i18n";
import "./turnActions.css";

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
  const loadFile = useCallback(async (path: string) => {
    const file = await loadTurnFile(client, taskId, artifact!, path);
    setArtifact(current => current?.data === artifact?.data && current?.diff ? { ...current,
      diff: { ...current.diff, files: current.diff.files.map(old => (old.newPath || old.oldPath) === path ? file : old) },
    } : current);
    return file;
  }, [client, taskId, artifact]);
  const unavailableCount = artifact?.diff?.files.filter(file => file.unavailableReason).length ?? 0;
  return <dialog ref={dialog} className="historical-turn-diff historical-turn-files" onCancel={onClose} aria-labelledby={titleId}>
    <header><h2 id={titleId} tabIndex={-1}>{t("historicalDiff.title")}</h2><button type="button" onClick={onClose} aria-label={t("historicalDiff.close")}>×</button></header>
    <div className="historical-turn-diff-content">{error ? <p role="alert">{error} <button type="button" onClick={() => setRetry(n => n + 1)}>{t("common.retry")}</button></p> : !artifact ? <LoadingStatus layout="panel">{t("historicalDiff.loading")}</LoadingStatus> : <>
      {artifact.data.attribution === "workspace_recovery_interval" && <p role="status">{t("historicalDiff.recoveryInterval")}</p>}
      {artifact.diff && <HistoricalDiffView key={`${metadata.artifactId}:${initialPath ?? ""}`} diff={artifact.diff} initialPath={initialPath} loadFile={loadFile} />}
      {unavailableCount > 0 && <p className="turn-review-notice">{t("historicalDiff.summary", { count: artifact.data.files.length, available: artifact.data.files.length - unavailableCount, unavailable: unavailableCount })}</p>}
      {!artifact.data.files.length && <p>{t("historicalDiff.empty")}</p>}
      {artifact.data.patch && <details><summary>{t("historicalDiff.rawPatch")}</summary>{artifact.data.patchTruncated && <p role="status">{t("historicalDiff.rawPartial")}</p>}<pre>{artifact.data.patch}</pre></details>}
    </>}</div>
  </dialog>;
}
