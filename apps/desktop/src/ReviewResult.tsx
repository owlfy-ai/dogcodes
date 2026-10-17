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
const severities = {critical: "严重", high: "高", medium: "中", low: "低"};
const severityOrder = ["critical", "high", "medium", "low"] as const;
const stages: Record<string, string> = {queued: "等待检查", reviewing: "初审中", verifying: "独立复核中", completed: "已完成", partial: "部分完成 · 查看原因", failed: "失败", skipped: "未执行", cancelled: "已取消", interrupted: "已中断"};
const verdicts = {confirmed: "AI 复核支持", refuted: "AI 复核不支持", uncertain: "待核实"};
const coverage = {reported: "已检查", partial: "部分原文缺失", not_reported: "未报告检查", unavailable: "原文不可用"};
// Known boilerplate from saved legacy reports; preserve specific failures and findings.
const legacyNotices: Record<string, string | null> = {
  "安全专项覆盖不完整；未报告、缺少依据或缺少上下文的项目不能视为通过。": "安全检查未完成。",
  "审查覆盖不完整，不能据此判定全部改动通过。": "审查未完成。",
  "安全检查仅分析冻结代码，未执行漏洞复现；风险等级为 Agent 的潜在影响评估，AI 复核支持不等于漏洞已实测确认。": null,
};
const fileLabel = (file: {path: string | null; rootLabel?: string | null}) => file.rootLabel && file.path && !file.path.startsWith("/") ? `${file.rootLabel.replace(/\/$/, "")}/${file.path}` : file.path;
export function ReviewResult({ client, run: savedRun, executor, allowRepair = true, onReviewFinding, onRetry, retryDisabled, retryTooltip }: {client: CoreClient; run: TurnReviewRun; executor?: ExecutorRef; allowRepair?: boolean; onReviewFinding?: (finding: ReviewFinding) => void; onRetry?: () => void; retryDisabled?: boolean; retryTooltip?: string}) {
  const run = useReviewStream(client, savedRun);
  const notices = (run.report?.warnings ?? []).map(warning => !run.workspaceReview && Object.hasOwn(legacyNotices, warning) ? legacyNotices[warning] : warning).filter((warning): warning is string => Boolean(warning));
  const activity = <ReviewActivity client={client} run={run} notices={notices}/>;
  const [selected, setSelected] = useState<number[]>([]);
  const [expanded, setExpanded] = useState<number[]>([]);
  const detailsId = useId();
  const selectable = allowRepair && !!run.report?.findings.length && !!client.reviewRepair && !["queued", "running", "cancelling"].includes(run.status);
  const [finding, setFinding] = useState<ReviewFinding | null>(null);
  const report = run.report;
  const failure = <ReviewFailure error={run.error ?? (run.status === "interrupted" ? "审查连接中断" : run.status === "failed" ? "审查未完成，请重试或更换 Agent。" : null)} onRetry={onRetry} retryDisabled={retryDisabled} retryTooltip={retryTooltip}/>;
  if (!report) return <>{failure}{activity}</>;
  const reported = report.files.filter(file => file.status === "reported").length;
  return <div className="review-report">
    {failure}
    {run.workflow && <details className="review-coverage" open={run.status === "running" || run.status === "queued" || run.status === "cancelling"}>
      <summary>分组审查：{run.workflow.groups.filter(group => group.status === "completed").length}/{run.workflow.groups.length} 组完成</summary>
      <ul>{run.workflow.groups.map((group, index) => <li key={group.id}>
        <span>第 {index + 1} 组 · {group.fileIds.length} 个文件 · {stages[group.status] ?? group.status}</span>
        <p>{group.fileIds.map(id => fileLabel(report.files.find(file => file.fileId === id) ?? {path:id})).join("、")}</p>
        {group.error && <p role="status">{group.error}</p>}
      </li>)}</ul>
    </details>}
    {!run.workspaceReview && <details className="review-coverage"><summary>文件覆盖：{reported}/{report.files.length}</summary>
      <ul>{report.files.map(file => <li key={file.fileId}><span>{fileLabel(file)} — {coverage[file.status]}</span><details><summary>本文件检查规则</summary><ul>{file.rules.map(rule => <li key={rule}>{rule}</li>)}</ul></details></li>)}</ul>
    </details>}
    {!run.workspaceReview && run.request.kind === "security" && <ReviewSecurityCoverage report={report}/>}
    {(report.structured || (run.workflow && !!report.findings.length)) && <>
      {report.findings.length
        ? <div className="review-summary">
          <strong>{report.findings.length} 项问题</strong>
          <span className="review-summary-counts">{severityOrder.map(level => {
            const total = report.findings.filter(item => item.severity === level).length;
            return total ? <span key={level} className={`review-severity ${level}`}>{severities[level]} {total}</span> : null;
          })}</span>
          {selectable && <label className="review-select-all"><input type="checkbox" aria-label="全选问题" checked={selected.length === report.findings.length} ref={input => { if (input) input.indeterminate = selected.length > 0 && selected.length < report.findings.length; }} onChange={event => setSelected(event.target.checked ? report.findings.map((_, index) => index) : [])}/>全选</label>}
        </div>
        : run.status === "completed" ? <p className="review-summary is-clean"><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><circle cx="8" cy="8" r="6.2"/><path d="m5.3 8.2 1.8 1.8 3.6-3.8"/></svg>本次审查未报告问题。</p>
          : <p className="turn-review-notice">审查尚未完成，不能据此判断是否存在问题。</p>}
      <div className="review-findings">{report.findings.map((item, index) => {
        const open = expanded.includes(index);
        return <article className="review-finding" data-severity={item.severity} data-open={open || undefined} data-selected={selected.includes(index) || undefined} key={`${index}:${item.fileId}`}>
          <div className="review-finding-head">
            {selectable && <input type="checkbox" aria-label={`选择问题：${item.title}`} checked={selected.includes(index)} onChange={event => setSelected(previous => event.target.checked ? [...previous, index] : previous.filter(i => i !== index))}/>}
            <button type="button" className="review-finding-toggle" aria-expanded={open} aria-controls={`${detailsId}-${index}`} onClick={() => setExpanded(previous => previous.includes(index) ? previous.filter(i => i !== index) : [...previous, index])}>
              <span className={`review-severity ${item.severity}`}>{run.request.kind === "security" ? `潜在${severities[item.severity]}风险` : severities[item.severity]}</span>
              <span className="review-finding-title">{item.title}</span>
              <svg className="review-finding-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m4 6 4 4 4-4"/></svg>
            </button>
          </div>
          <div className="review-finding-meta">
            <button type="button" className="review-location" disabled={onReviewFinding ? !item.path : !item.locationVerified || !client.reviewSource} data-tooltip={reviewFindingPath(item)} onClick={() => onReviewFinding ? onReviewFinding(item) : setFinding(item)}>
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" aria-hidden="true"><path d="M9 1.5H3.5v13h9v-9L9 1.5Zm0 0v4h3.5M5.5 8h5m-5 3h5"/></svg>
              <span className="review-location-name">{(item.path ?? item.fileId).replaceAll("\\", "/").split("/").pop()}</span>
              <span className="review-location-lines">{item.side === "before" ? "改动前 · " : ""}{item.startLine}{item.endLine !== item.startLine ? `–${item.endLine}` : ""} 行</span>
            </button>
            {item.verification && <span className={`review-verdict ${item.verification.verdict}`}>{verdicts[item.verification.verdict]}</span>}
          </div>
          <div className="review-finding-details" id={`${detailsId}-${index}`} hidden={!open}>
            <p className="review-finding-prose">{item.description}</p>
            {item.verification && <div className={`review-verification ${item.verification.verdict}`}><strong>复核说明</strong><p>{item.verification.reason}</p>{!!item.verification.citations?.length && <details><summary>代码证据</summary>{item.verification.citations.map((citation, index) => <div key={index}><p>{citation.path ? fileLabel({path: citation.path, rootLabel: citation.rootLabel}) : report.files.find(file => file.fileId === citation.sourceId)?.path ?? "关联文件"} · {citation.side === "before" ? "改动前" : "改动后"} {citation.startLine}–{citation.endLine} 行</p><pre>{citation.evidence}</pre></div>)}</details>}</div>}
            {!run.workspaceReview && run.request.kind === "security" && <ReviewSecurityFinding finding={item}/>}
            {item.evidence && <pre><code>{item.evidence}</code></pre>}
            {item.suggestion && <p className="review-finding-prose review-finding-suggestion"><strong>建议</strong>{item.suggestion}</p>}
          </div>
        </article>;
      })}</div>
    </>}
    {selectable && <ReviewRepairControls client={client} run={run} executor={executor} selected={selected} clearSelection={() => setSelected([])} renderReport={child => <ReviewResult client={client} run={child} executor={executor} onReviewFinding={onReviewFinding}/>}/>}
    {activity}
    {finding && <ReviewFindingDiff client={client} run={run} finding={finding} onClose={() => setFinding(null)}/>}
  </div>;
}
