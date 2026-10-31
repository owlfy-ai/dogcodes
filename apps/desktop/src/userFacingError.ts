import { translate as t } from "./i18n";
/** Keep actionable product messages, without exposing transport errors or stack traces. */
export function userFacingError(cause: unknown, fallback: string): string {
  const message = cause instanceof Error ? cause.message : typeof cause === "string" ? cause : "";
  if (/permission denied|EACCES|EPERM/i.test(message)) return t("agentUi.permissionDenied");
  if (/ENOSPC|disk full/i.test(message)) return t("agentUi.diskFull");
  if (/timed? ?out|timeout|ECONN|network|fetch failed/i.test(message)) return t("userError.network");
  if (message.length <= 200 && /[\u3400-\u9fff]/u.test(message) && !/[\n{}]|\b(?:panic|stack|IPC|RPC|SQL|Error:)\b/i.test(message)) return message;
  return fallback;
}
