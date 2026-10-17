import { approvalPresentation, type ApprovalPresentation } from "./approvalPresentation";

export function ApprovalDetails({ summary, presentation }: { summary?: string; presentation?: ApprovalPresentation }) {
  const view = presentation ?? approvalPresentation(summary ?? "");
  return <div className="approval-details">
    {view.warning && <p role="alert" className="approval-warning">{view.warning}</p>}
    {view.sections.map((section, index) => section.expandable ? <details className="approval-change" key={index} open>
      <summary>{section.label}</summary><pre aria-label={section.label}><code>{section.text}</code></pre>
    </details> : <div className="approval-section" key={index}>
      {view.sections.length > 1 && <span className="approval-section-label">{section.label}</span>}
      <pre aria-label={section.label}><code>{section.text}</code></pre>
    </div>)}
  </div>;
}
