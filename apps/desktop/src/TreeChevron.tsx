/** Rotate one centered shape so expanding a directory never changes its baseline. */
export function TreeChevron({ expanded }: { expanded: boolean }) {
  return <svg className="review-chevron" data-expanded={expanded} width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
    <path d="m6 4 4 4-4 4" />
  </svg>;
}
