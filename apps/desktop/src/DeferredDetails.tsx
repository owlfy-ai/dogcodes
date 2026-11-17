import { useState, type ReactNode } from "react";

/** Mount expensive disclosure content on first open, then retain its state. */
export function DeferredDetails({ className, turnId, summary, children, onFirstOpen }: {
  className: string;
  /** Lets a search result open the turn holding its message. */
  turnId?: string;
  summary: ReactNode;
  children: () => ReactNode;
  onFirstOpen?: () => void;
}) {
  const [visited, setVisited] = useState(false);
  return <details className={className} data-turn-process={turnId} onToggle={event => {
    if (event.target === event.currentTarget && event.currentTarget.open && !visited) { setVisited(true); onFirstOpen?.(); }
  }}>
    {summary}
    {visited && children()}
  </details>;
}
