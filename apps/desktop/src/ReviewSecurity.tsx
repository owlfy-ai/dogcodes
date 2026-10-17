import type { ReviewFinding, ReviewReport, SecurityCitation, SecurityEvidence } from "@dogcodes/protocol";

const statuses = {reviewed: "已检查", not_applicable: "不适用", needs_context: "依据不足 / 缺少上下文", not_reported: "未报告检查"};
const roles: Record<string, string> = {entry: "输入入口", propagation: "传播路径", sink: "危险操作 / 边界效果"};

function Citation({citation}: {citation: SecurityCitation}) {
  const path = citation.path && citation.rootLabel && !citation.path.startsWith("/")
    ? `${citation.rootLabel.replace(/\/$/, "")}/${citation.path}` : citation.path;
  return <div><p>{path ?? citation.sourceId} · {citation.side === "before" ? "改动前" : "改动后"} {citation.startLine}–{citation.endLine} 行{!path && " · 引用待核实"}</p><pre>{citation.evidence}</pre></div>;
}

export function ReviewSecurityCoverage({report}: {report: ReviewReport}) {
  const files = report.files.filter(file => file.securityChecks?.length);
  if (!files.length) return null;
  const checks = files.flatMap(file => file.securityChecks ?? []);
  const reviewed = checks.filter(check => check.status === "reviewed").length;
  const excluded = checks.filter(check => check.status === "not_applicable").length;
  return <details className="review-coverage review-security-coverage">
    <summary>安全专项：{reviewed} 项已检查 · {excluded} 项不适用 · {checks.length - reviewed - excluded} 项待补充</summary>
    <ul>{files.map(file => <li key={file.fileId}>
      <span>{file.rootLabel && !file.path.startsWith("/") ? `${file.rootLabel.replace(/\/$/, "")}/` : ""}{file.path}</span>
      <ul>{file.securityChecks!.map(check => <li key={check.attackClass}>
        <details><summary>{check.label} — {statuses[check.status]}</summary>
          <p className="review-finding-prose">{check.reason}</p>
          {check.citations.map((citation, i) => <Citation key={i} citation={citation}/>)}
        </details>
      </li>)}</ul>
    </li>)}</ul>
  </details>;
}

function EvidenceDetails({evidence, title}: {evidence: SecurityEvidence; title: string}) {
  return <details><summary>{title}</summary>
      <dl className="review-security-fields">
        <dt>攻击者与已有能力</dt><dd>{evidence.attacker}</dd>
        <dt>信任边界</dt><dd>{evidence.boundary}</dd>
        <dt>触发条件</dt><dd>{evidence.preconditions}</dd>
        <dt>现有防护与不足</dt><dd>{evidence.controls}</dd>
        <dt>潜在影响</dt><dd>{evidence.impact}</dd>
      </dl>
      <ol>{evidence.trace.map((step, i) => <li key={i}><strong>{roles[step.role] ?? "路径节点"}</strong><p>{step.explanation}</p><Citation citation={step}/></li>)}</ol>
      {!!evidence.blockers.length && <><p>尚未解决的条件</p><ul>{evidence.blockers.map((blocker, i) => <li key={i}>{blocker}</li>)}</ul></>}
      <p className="review-finding-prose">后续验证建议（未执行）：{evidence.validationPlan}</p>
    </details>;
}

export function ReviewSecurityFinding({finding}: {finding: ReviewFinding}) {
  const revised = finding.verification?.securityEvidence;
  const security = revised ?? finding.security;
  if (!security) return null;
  const review = finding.verification?.securityReview;
  return <div className="review-security-finding">
    <p className="turn-review-notice">{security.evidenceStatus !== "source_checked" && "安全证据待补充 · "}未执行漏洞复现</p>
    {security.notes.map((note, i) => <p className="turn-review-notice" key={i}>{note}</p>)}
    {security.evidence && <EvidenceDetails evidence={security.evidence} title={revised ? "复核补充的证据与验证条件" : "安全证据与验证条件"}/>}
    {revised && finding.security && <details><summary>保留的初审证据（复核前）</summary>
      {finding.security.notes.map((note, i) => <p key={i}>{note}</p>)}
      {finding.security.evidence && <EvidenceDetails evidence={finding.security.evidence} title="查看初审证据与当时未解条件"/>}
    </details>}
    {review && <details><summary>独立安全复核依据</summary><dl className="review-security-fields">
      <dt>边界与前提</dt><dd>{review.boundaryAnalysis}</dd>
      <dt>防护反证</dt><dd>{review.controlAnalysis}</dd>
      <dt>影响范围</dt><dd>{review.impactAnalysis}</dd>
    </dl>{!!review.unresolved.length && <><p>复核仍待确认</p><ul>{review.unresolved.map((item, i) => <li key={i}>{item}</li>)}</ul></>}</details>}
  </div>;
}
