import { userFacingError } from "./userFacingError";

const transport = /keepalive|ECONN|ENOTFOUND|EAI_AGAIN|ETIMEDOUT|socket hang up|network error|fetch failed|connection (?:reset|refused|closed|is closed|lost|dropped)|HTTP\/2 stream (?:closed|reset)|driver_disconnected|agent_connection_failed|审查连接中断|与 Agent 的连接已中断/i;
export const isReviewTransportErrorLine = (text: string) => /^\s*(?:Error|RetriableError|ConnectError|NetworkError|TypeError):/.test(text) && (transport.test(text) || /timed out|timeout/i.test(text));

export function reviewFailureMessage(error: string): string {
  if (transport.test(error)) return "与审查 Agent 的连接中断，审查尚未完成。请检查网络或代理后重试。";
  if (/timed out|timeout|超时/i.test(error)) return "审查等待超时，尚未生成完整结论。请稍后重试或更换 Agent。";
  return userFacingError(error, "审查未完成，请重试或更换 Agent。");
}

export function ReviewFailure({ error, onRetry, retryDisabled, retryTooltip }: {
  error: string | null; onRetry?: () => void; retryDisabled?: boolean; retryTooltip?: string;
}) {
  if (!error) return null;
  const message = reviewFailureMessage(error);
  return <div className="review-failure">
    <p role="alert">{message}</p>
    {onRetry && <button type="button" disabled={retryDisabled} data-tooltip={retryTooltip} onClick={onRetry}>重试审查</button>}
    {message !== error && <details><summary>查看错误详情</summary><pre>{error}</pre></details>}
  </div>;
}
