/** Keep actionable product messages, without exposing transport errors or stack traces. */
export function userFacingError(cause: unknown, fallback: string): string {
  const message = cause instanceof Error ? cause.message : typeof cause === "string" ? cause : "";
  if (/permission denied|EACCES|EPERM/i.test(message)) return "没有访问权限，请检查文件夹的访问权限后重试。";
  if (/ENOSPC|disk full/i.test(message)) return "磁盘空间不足，请释放一些空间后重试。";
  if (/timed? ?out|timeout|ECONN|network|fetch failed/i.test(message)) return "连接暂时不可用，请稍后重试。";
  if (message.length <= 200 && /[\u3400-\u9fff]/u.test(message) && !/[\n{}]|\b(?:panic|stack|IPC|RPC|SQL|Error:)\b/i.test(message)) return message;
  return fallback;
}
