import type { ReactNode } from "react";
import "./loadingStatus.css";

/**
 * Quiet feedback for pending reads: inline as bouncing dots with shimmering text, in a
 * panel as skeleton lines. The running dog is reserved for main areas (ActivityLoader).
 * Errors and empty results stay separate.
 */
export function LoadingStatus({ children, layout = "inline", className = "" }: {
  children: ReactNode;
  layout?: "inline" | "panel";
  className?: string;
}) {
  return <span className={`loading-status loading-status--${layout} ${className}`} role="status" aria-live="polite">
    {layout === "panel"
      ? <span className="loading-skeleton" aria-hidden="true"><i /><i /><i /><i /></span>
      : <span className="loading-dots" aria-hidden="true"><i /><i /><i /></span>}
    <span className="loading-status-label">{children}</span>
  </span>;
}
