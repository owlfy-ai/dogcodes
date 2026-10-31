import type { ReviewFinding, ReviewReport, SecurityCitation, SecurityEvidence } from "@dogcodes/protocol";
import { translate as t } from "./i18n";

const statuses = {reviewed: "reviewed", not_applicable: "notApplicable", needs_context: "needsContext", not_reported: "notReported"};
const roles: Record<string, string> = {entry: "entry", propagation: "propagation", sink: "sink"};

function Citation({citation}: {citation: SecurityCitation}) {
  const path = citation.path && citation.rootLabel && !citation.path.startsWith("/")
    ? `${citation.rootLabel.replace(/\/$/, "")}/${citation.path}` : citation.path;
  return <div><p>{path ?? citation.sourceId} · {t(citation.side === "before" ? "reviewResult.citationBefore" : "reviewResult.citationAfter", { start: citation.startLine, end: citation.endLine })}{!path && ` · ${t("reviewSecurity.citationUnverified")}`}</p><pre>{citation.evidence}</pre></div>;
}

export function ReviewSecurityCoverage({report}: {report: ReviewReport}) {
  const files = report.files.filter(file => file.securityChecks?.length);
  if (!files.length) return null;
  const checks = files.flatMap(file => file.securityChecks ?? []);
  const reviewed = checks.filter(check => check.status === "reviewed").length;
  const excluded = checks.filter(check => check.status === "not_applicable").length;
  return <details className="review-coverage review-security-coverage">
    <summary>{t("reviewSecurity.summary", { reviewed, excluded, pending: checks.length - reviewed - excluded })}</summary>
    <ul>{files.map(file => <li key={file.fileId}>
      <span>{file.rootLabel && !file.path.startsWith("/") ? `${file.rootLabel.replace(/\/$/, "")}/` : ""}{file.path}</span>
      <ul>{file.securityChecks!.map(check => <li key={check.attackClass}>
        <details><summary>{check.label} — {t(`reviewSecurity.status.${statuses[check.status]}`)}</summary>
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
        <dt>{t("reviewSecurity.attacker")}</dt><dd>{evidence.attacker}</dd>
        <dt>{t("reviewSecurity.boundary")}</dt><dd>{evidence.boundary}</dd>
        <dt>{t("reviewSecurity.preconditions")}</dt><dd>{evidence.preconditions}</dd>
        <dt>{t("reviewSecurity.controls")}</dt><dd>{evidence.controls}</dd>
        <dt>{t("reviewSecurity.impact")}</dt><dd>{evidence.impact}</dd>
      </dl>
      <ol>{evidence.trace.map((step, i) => <li key={i}><strong>{t(`reviewSecurity.role.${roles[step.role] ?? "node"}`)}</strong><p>{step.explanation}</p><Citation citation={step}/></li>)}</ol>
      {!!evidence.blockers.length && <><p>{t("reviewSecurity.blockers")}</p><ul>{evidence.blockers.map((blocker, i) => <li key={i}>{blocker}</li>)}</ul></>}
      <p className="review-finding-prose">{t("reviewSecurity.validationPlan", { plan: evidence.validationPlan })}</p>
    </details>;
}

export function ReviewSecurityFinding({finding}: {finding: ReviewFinding}) {
  const revised = finding.verification?.securityEvidence;
  const security = revised ?? finding.security;
  if (!security) return null;
  const review = finding.verification?.securityReview;
  return <div className="review-security-finding">
    <p className="turn-review-notice">{security.evidenceStatus !== "source_checked" && `${t("reviewSecurity.evidencePending")} · `}{t("reviewSecurity.notReproduced")}</p>
    {security.notes.map((note, i) => <p className="turn-review-notice" key={i}>{note}</p>)}
    {security.evidence && <EvidenceDetails evidence={security.evidence} title={t(revised ? "reviewSecurity.revisedEvidence" : "reviewSecurity.evidence")}/>}
    {revised && finding.security && <details><summary>{t("reviewSecurity.initialEvidence")}</summary>
      {finding.security.notes.map((note, i) => <p key={i}>{note}</p>)}
      {finding.security.evidence && <EvidenceDetails evidence={finding.security.evidence} title={t("reviewSecurity.viewInitialEvidence")}/>}
    </details>}
    {review && <details><summary>{t("reviewSecurity.reviewBasis")}</summary><dl className="review-security-fields">
      <dt>{t("reviewSecurity.reviewBoundary")}</dt><dd>{review.boundaryAnalysis}</dd>
      <dt>{t("reviewSecurity.reviewControls")}</dt><dd>{review.controlAnalysis}</dd>
      <dt>{t("reviewSecurity.reviewImpact")}</dt><dd>{review.impactAnalysis}</dd>
    </dl>{!!review.unresolved.length && <><p>{t("reviewSecurity.reviewUnresolved")}</p><ul>{review.unresolved.map((item, i) => <li key={i}>{item}</li>)}</ul></>}</details>}
  </div>;
}
