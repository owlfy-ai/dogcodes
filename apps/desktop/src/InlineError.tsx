import type { ReactNode } from "react";
import "./inlineError.css";
import { translate as t } from "./i18n";

/** Shared inline failure row. Pass text already cleaned by `userFacingError`; never raw exceptions. */
export function InlineError({ children, onRetry, retryLabel = t("common.retry"), role = "alert", className = "" }: {
  children: ReactNode;
  onRetry?: () => void;
  retryLabel?: string;
  role?: "alert" | "status";
  className?: string;
}) {
  return <p className={`inline-error ${className}`.trim()} role={role}>
    <span className="inline-error-text">{children}</span>
    {onRetry && <button type="button" className="inline-error-retry" onClick={onRetry}>{retryLabel}</button>}
  </p>;
}
