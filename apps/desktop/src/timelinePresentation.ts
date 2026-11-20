export type TimelineItemRole = "user" | "assistant" | "tool" | "system";

export function timelineItemRole(kind: string): TimelineItemRole {
  if (kind === "user_message" || kind === "followup_queued" || kind === "followup_failed" || kind === "steer_rejected" || kind === "steer_unconfirmed") return "user";
  if (kind === "assistant_message") return "assistant";
  if (kind.startsWith("tool.")) return "tool";
  return "system";
}
