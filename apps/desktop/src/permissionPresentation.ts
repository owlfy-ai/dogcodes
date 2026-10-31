import type { PermissionCatalog } from "@dogcodes/protocol";
import { translate } from "./i18n";

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

// Catalogs the core builds itself carry Chinese descriptions; translate those
// known modes and keep descriptions that agents report natively.
const coreDescriptions: Record<string, Record<string, string>> = {
  codex: { "read-only": "codexReadOnly", default: "codexDefault", "full-access": "codexFullAccess" },
  "claude-code": { plan: "claudePlan", default: "claudeDefault", acceptEdits: "claudeAcceptEdits", bypassPermissions: "claudeBypass" },
  antigravity: { "desktop-approvals": "antigravityApprovals", "dangerously-skip-permissions": "antigravitySkip" },
};

// Only normalize known policies. Preserve native text for new ACP modes.
export function permissionPresentation(providerId: string, mode: Mode): Mode {
  const kind = permissionKind(providerId, mode);
  const description = coreDescriptions[providerId === "claude" ? "claude-code" : providerId]?.[mode.id];
  return {
    ...mode,
    ...(kind ? { label: translate(`permissionPicker.mode.${kind}`) } : {}),
    ...(description ? { description: translate(`permissionPicker.description.${description}`) } : {}),
  };
}
