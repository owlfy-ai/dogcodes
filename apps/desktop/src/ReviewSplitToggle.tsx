import { translate as t } from "./i18n";

export function ReviewSplitToggle({ split, onToggle }: { split: boolean; onToggle: () => void }) {
  return <button type="button" className="review-icon-button review-split-toggle" aria-label={t("reviewPanel.split")}
    data-tooltip={t(split ? "reviewPanel.unified" : "reviewPanel.split")} aria-pressed={split} onClick={onToggle}>
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="2" y="2" width="12" height="12" rx="2" /><path d="M8 2v12M4 5h1m-1 3h1m6-3h1m-1 3h1" /></svg>
  </button>;
}
