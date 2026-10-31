import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

const transport = /keepalive|ECONN|ENOTFOUND|EAI_AGAIN|ETIMEDOUT|socket hang up|network error|fetch failed|connection (?:reset|refused|closed|is closed|lost|dropped)|HTTP\/2 stream (?:closed|reset)|driver_disconnected|agent_connection_failed|审查连接中断|与 Agent 的连接已中断/i;
export const isReviewTransportErrorLine = (text: string) => /^\s*(?:Error|RetriableError|ConnectError|NetworkError|TypeError):/.test(text) && (transport.test(text) || /timed out|timeout/i.test(text));

export function reviewFailureMessage(error: string): string {
  if (transport.test(error)) return t("reviewFailure.transport");
  if (/timed out|timeout|超时/i.test(error)) return t("reviewFailure.timeout");
  return userFacingError(error, t("reviewResult.failed"));
}

export function ReviewFailure({ error, onRetry, retryDisabled, retryTooltip }: {
  error: string | null; onRetry?: () => void; retryDisabled?: boolean; retryTooltip?: string;
}) {
  if (!error) return null;
  const message = reviewFailureMessage(error);
  return <div className="review-failure">
    <p role="alert">{message}</p>
    {onRetry && <button type="button" disabled={retryDisabled} data-tooltip={retryTooltip} onClick={onRetry}>{t("reviewFailure.retry")}</button>}
    {message !== error && <details><summary>{t("gitMenu.errorDetails")}</summary><pre>{error}</pre></details>}
  </div>;
}
