import { useState, type ReactNode } from "react";

/** Mount expensive disclosure content on first open, then retain its state. */
export function DeferredDetails({ className, summary, children }: {
  className: string;
  summary: ReactNode;
  children: () => ReactNode;
}) {
  const [visited, setVisited] = useState(false);
  return <details className={className} onToggle={event => {
    if (event.target === event.currentTarget && event.currentTarget.open) setVisited(true);
  }}>
    {summary}
    {visited && children()}
  </details>;
}
