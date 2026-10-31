import { useId, useState } from "react";
import { ReviewActivity } from "./ReviewActivity";
import { useReviewStream } from "./reviewStream";
import { reviewFindingPath } from "./reviewTarget";
import type { CoreClient, ReviewFinding, TurnReviewRun, ExecutorRef } from "@dogcodes/protocol";
import { ReviewFindingDiff } from "./ReviewFindingDiff";
import "./reviewResult.css";
import { ReviewRepairControls } from "./ReviewRepairControls";
import { ReviewSecurityCoverage, ReviewSecurityFinding } from "./ReviewSecurity";
import { ReviewFailure } from "./reviewFailure";
import { translate as t } from "./i18n";
const severities = {critical: "critical", high: "high", medium: "medium", low: "low"};
const severityOrder = ["critical", "high", "medium", "low"] as const;
const stages: Record<string, string> = {queued: "queued", reviewing: "reviewing", verifying: "verifying", completed: "completed", partial: "partial", failed: "failed", skipped: "skipped", cancelled: "cancelled", interrupted: "interrupted"};
const verdicts = {confirmed: "confirmed", refuted: "refuted", uncertain: "uncertain"};
const coverage = {reported: "reported", partial: "partial", not_reported: "notReported", unavailable: "unavailable"};
// Known boilerplate from saved legacy reports (matched as data, mapped to translation
// keys); preserve specific failures and findings.
const legacyNotices: Record<string, string | null> = {
  "安全专项覆盖不完整；未报告、缺少依据或缺少上下文的项目不能视为通过。": "reviewResult.securityIncomplete",
  "审查覆盖不完整，不能据此判定全部改动通过。": "reviewResult.reviewIncomplete",
  "安全检查仅分析冻结代码，未执行漏洞复现；风险等级为 Agent 的潜在影响评估，AI 复核支持不等于漏洞已实测确认。": null,
};
const fileLabel = (file: {path: string | null; rootLabel?: string | null}) => file.rootLabel && file.path && !file.path.startsWith("/") ? `${file.rootLabel.replace(/\/$/, "")}/${file.path}` : file.path;
export function ReviewResult({ client, run: savedRun, executor, allowRepair = true, onReviewFinding, onRetry, retryDisabled, retryTooltip }: {client: CoreClient; run: TurnReviewRun; executor?: ExecutorRef; allowRepair?: boolean; onReviewFinding?: (finding: ReviewFinding) => void; onRetry?: () => void; retryDisabled?: boolean; retryTooltip?: string}) {
  const run = useReviewStream(client, savedRun);
  const notices = (run.report?.warnings ?? []).map(warning => !run.workspaceReview && Object.hasOwn(legacyNotices, warning) ? legacyNotices[warning] && t(legacyNotices[warning]) : warning).filter((warning): warning is string => Boolean(warning));
  const activity = <ReviewActivity client={client} run={run} notices={notices}/>;
  const [selected, setSelected] = useState<number[]>([]);
  const [expanded, setExpanded] = useState<number[]>([]);
  const detailsId = useId();
  const selectable = allowRepair && !!run.report?.findings.length && !!client.reviewRepair && !["queued", "running", "cancelling"].includes(run.status);
  const [finding, setFinding] = useState<ReviewFinding | null>(null);
  const report = run.report;
  const failure = <ReviewFailure error={run.error ?? (run.status === "interrupted" ? t("reviewResult.interrupted") : run.status === "failed" ? t("reviewResult.failed") : null)} onRetry={onRetry} retryDisabled={retryDisabled} retryTooltip={retryTooltip}/>;
  if (!report) return <>{failure}{activity}</>;
  const reported = report.files.filter(file => file.status === "reported").length;
  return <div className="review-report">
    {failure}
    {run.workflow && <details className="review-coverage" open={run.status === "running" || run.status === "queued" || run.status === "cancelling"}>
      <summary>{t("reviewResult.groups", { done: run.workflow.groups.filter(group => group.status === "completed").length, total: run.workflow.groups.length })}</summary>
      <ul>{run.workflow.groups.map((group, index) => <li key={group.id}>
        <span>{t("reviewResult.group", { index: index + 1, files: group.fileIds.length, status: stages[group.status] ? t(`reviewResult.stage.${stages[group.status]}`) : group.status })}</span>
        <p>{group.fileIds.map(id => fileLabel(report.files.find(file => file.fileId === id) ?? {path:id})).join("、")}</p>
        {group.error && <p role="status">{group.error}</p>}
      </li>)}</ul>
    </details>}
    {!run.workspaceReview && <details className="review-coverage"><summary>{t("reviewResult.coverage", { reported, total: report.files.length })}</summary>
      <ul>{report.files.map(file => <li key={file.fileId}><span>{fileLabel(file)} — {t(`reviewResult.coverageStatus.${coverage[file.status]}`)}</span><details><summary>{t("reviewResult.fileRules")}</summary><ul>{file.rules.map(rule => <li key={rule}>{rule}</li>)}</ul></details></li>)}</ul>
    </details>}
    {!run.workspaceReview && run.request.kind === "security" && <ReviewSecurityCoverage report={report}/>}
    {(report.structured || (run.workflow && !!report.findings.length)) && <>
      {report.findings.length
        ? <div className="review-summary">
          <strong>{t("reviewResult.findingCount", { count: report.findings.length })}</strong>
          <span className="review-summary-counts">{severityOrder.map(level => {
            const total = report.findings.filter(item => item.severity === level).length;
            return total ? <span key={level} className={`review-severity ${level}`}>{severities[level]} {total}</span> : null;
          })}</span>
          {selectable && <label className="review-select-all"><input type="checkbox" aria-label={t("reviewResult.selectAll")} checked={selected.length === report.findings.length} ref={input => { if (input) input.indeterminate = selected.length > 0 && selected.length < report.findings.length; }} onChange={event => setSelected(event.target.checked ? report.findings.map((_, index) => index) : [])}/>{t("reviewResult.selectAllShort")}</label>}
        </div>
        : run.status === "completed" ? <p className="review-summary is-clean"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><circle cx="8" cy="8" r="6.2"/><path d="m5.3 8.2 1.8 1.8 3.6-3.8"/></svg>{t("reviewResult.clean")}</p>
          : <p className="turn-review-notice">{t("reviewResult.notFinished")}</p>}
      <div className="review-findings">{report.findings.map((item, index) => {
        const open = expanded.includes(index);
        return <article className="review-finding" data-severity={item.severity} data-open={open || undefined} data-selected={selected.includes(index) || undefined} key={`${index}:${item.fileId}`}>
          <div className="review-finding-head">
            {selectable && <input type="checkbox" aria-label={t("reviewResult.selectFinding", { title: item.title })} checked={selected.includes(index)} onChange={event => setSelected(previous => event.target.checked ? [...previous, index] : previous.filter(i => i !== index))}/>}
            <button type="button" className="review-finding-toggle" aria-expanded={open} aria-controls={`${detailsId}-${index}`} onClick={() => setExpanded(previous => previous.includes(index) ? previous.filter(i => i !== index) : [...previous, index])}>
              <span className={`review-severity ${item.severity}`}>{run.request.kind === "security" ? t(`reviewResult.potential.${severities[item.severity]}`) : t(`reviewResult.severity.${severities[item.severity]}`)}</span>
              <span className="review-finding-title">{item.title}</span>
              <svg className="review-finding-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m4 6 4 4 4-4"/></svg>
            </button>
          </div>
          <div className="review-finding-meta">
            <button type="button" className="review-location" disabled={onReviewFinding ? !item.path : !item.locationVerified || !client.reviewSource} data-tooltip={reviewFindingPath(item)} onClick={() => onReviewFinding ? onReviewFinding(item) : setFinding(item)}>
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" aria-hidden="true"><path d="M9 1.5H3.5v13h9v-9L9 1.5Zm0 0v4h3.5M5.5 8h5m-5 3h5"/></svg>
              <span className="review-location-name">{(item.path ?? item.fileId).replaceAll("\\", "/").split("/").pop()}</span>
              <span className="review-location-lines">{item.side === "before" ? `${t("reviewResult.before")} · ` : ""}{t("reviewResult.lines", { lines: `${item.startLine}${item.endLine !== item.startLine ? `–${item.endLine}` : ""}` })}</span>
            </button>
            {item.verification && <span className={`review-verdict ${item.verification.verdict}`}>{t(`reviewResult.verdict.${verdicts[item.verification.verdict]}`)}</span>}
          </div>
          <div className="review-finding-details" id={`${detailsId}-${index}`} hidden={!open}>
            <p className="review-finding-prose">{item.description}</p>
            {item.verification && <div className={`review-verification ${item.verification.verdict}`}><strong>{t("reviewResult.verification")}</strong><p>{item.verification.reason}</p>{!!item.verification.citations?.length && <details><summary>{t("reviewResult.codeEvidence")}</summary>{item.verification.citations.map((citation, index) => <div key={index}><p>{citation.path ? fileLabel({path: citation.path, rootLabel: citation.rootLabel}) : report.files.find(file => file.fileId === citation.sourceId)?.path ?? t("reviewResult.relatedFile")} · {t(citation.side === "before" ? "reviewResult.citationBefore" : "reviewResult.citationAfter", { start: citation.startLine, end: citation.endLine })}</p><pre>{citation.evidence}</pre></div>)}</details>}</div>}
            {!run.workspaceReview && run.request.kind === "security" && <ReviewSecurityFinding finding={item}/>}
            {item.evidence && <pre><code>{item.evidence}</code></pre>}
            {item.suggestion && <p className="review-finding-prose review-finding-suggestion"><strong>{t("reviewResult.suggestion")}</strong>{item.suggestion}</p>}
          </div>
        </article>;
      })}</div>
    </>}
    {selectable && <ReviewRepairControls client={client} run={run} executor={executor} selected={selected} clearSelection={() => setSelected([])} renderReport={child => <ReviewResult client={client} run={child} executor={executor} onReviewFinding={onReviewFinding}/>}/>}
    {activity}
    {finding && <ReviewFindingDiff client={client} run={run} finding={finding} onClose={() => setFinding(null)}/>}
  </div>;
}
