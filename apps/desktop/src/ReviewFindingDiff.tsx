import { LoadingStatus } from "./LoadingStatus";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { CoreClient, ReviewFinding, ReviewSource, TurnReviewRun } from "@dogcodes/protocol";
import { focusDialogTitle } from "./dialogFocus";
import { ReviewText } from "./ReviewText";
import { sourceDiff } from "./reviewSourceDiff";
import "./turnActions.css";
import { translate as t } from "./i18n";

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
    if (!client.reviewSource) { setError(t("findingDiff.unsupported")); return; }
    void client.reviewSource(run.request.taskId, run.request.runId, finding.fileId).then(value => {
      if (!valid) return;
      if (value.fileId !== finding.fileId || value.rootId !== finding.rootId || value.path !== finding.path) { setError(t("findingDiff.sourceMismatch")); return; }
      setSource(value);
    }, () => { if (valid) setError(t("findingDiff.loadFailed")); });
    return () => { valid = false; };
  }, [client, run.request.taskId, run.request.runId, finding, retry]);
  const diff = useMemo(() => source ? sourceDiff(source) : null, [source]);
  const excerpt = source?.[finding.side]?.split(/\r?\n/).slice(Math.max(0, finding.startLine - 6), finding.endLine + 5);
  return <dialog ref={dialog} className="historical-turn-diff review-finding-diff" aria-labelledby={titleId} onCancel={onClose}>
    <header><h2 tabIndex={-1} id={titleId}>{t("findingDiff.title")}</h2><button type="button" aria-label={t("findingDiff.close")} onClick={onClose}>×</button></header>
    <p>{finding.path} · {t(finding.side === "before" ? "reviewResult.citationBefore" : "reviewResult.citationAfter", { start: finding.startLine, end: finding.endLine })}</p>
    <p>{new Date(run.createdAt).toLocaleString()}</p>
    {error ? <p role="alert">{error}<button type="button" onClick={() => setRetry(n => n + 1)}>{t("common.retry")}</button></p> : !source ? <LoadingStatus layout="panel">{t("findingDiff.loading")}</LoadingStatus> : diff ?
      <ReviewText diff={diff} split={false} path={source.path} focusLine={{ side: finding.side, start: finding.startLine, end: finding.endLine }}/> :
      <><p role="status">{t("findingDiff.excerptOnly")}</p><pre>{excerpt?.map((line, index) => `${Math.max(1, finding.startLine - 5) + index}  ${line}`).join("\n") ?? t("findingDiff.sourceUnavailable")}</pre></>}
  </dialog>;
}
