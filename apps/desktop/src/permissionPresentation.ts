import type { PermissionCatalog } from "@dogcodes/protocol";

type Mode = PermissionCatalog["modes"][number];
type Icon = "ask" | "edit" | "plan" | "full" | "read" | "auto" | "configured";

// IDs belong to each provider: notably Kimi's auto and yolo have different
// meanings from similarly named modes in other agents.
const providerPolicies: Record<string, Record<string, Icon>> = {
  cursor: { default: "ask", "full-access": "full" },
  codex: { "read-only": "read", default: "edit", "full-access": "full" },
  "claude-code": { default: "ask", acceptEdits: "edit", plan: "plan", auto: "auto", bypassPermissions: "full" },
  grok: { ask: "ask", auto: "auto", "always-approve": "full" },
  zcode: { build: "ask", edit: "edit", plan: "plan", yolo: "full" },
  kimi: { default: "ask", yolo: "edit", plan: "plan", auto: "full" },
  kiro: { off: "ask", on: "full" },
  opencode: { build: "configured", plan: "plan" },
  antigravity: { configured: "configured", "desktop-approvals": "ask", "dangerously-skip-permissions": "full" },
};

export function permissionKind(providerId: string, mode?: Mode): Icon | undefined {
  const provider = providerId === "claude" ? "claude-code" : providerId;
  return mode ? providerPolicies[provider]?.[mode.id] : undefined;
}

const labels: Record<Icon, string> = {
  ask: "操作前确认",
  edit: "按需确认",
  plan: "计划模式",
  full: "完全访问",
  read: "只读模式",
  auto: "自动判断",
  configured: "原生规则",
};

// Only normalize known policies. Preserve native text for new ACP modes.
export function permissionPresentation(providerId: string, mode: Mode): Mode {
  const kind = permissionKind(providerId, mode);
  return kind ? { ...mode, label: labels[kind] } : mode;
}
