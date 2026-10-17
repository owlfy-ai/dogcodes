export type TimelineItemRole = "user" | "assistant" | "tool" | "system";

export function timelineItemRole(kind: string): TimelineItemRole {
  if (kind === "user_message") return "user";
  if (kind === "assistant_message") return "assistant";
  if (kind.startsWith("tool.")) return "tool";
  return "system";
}
