import type { PermissionCatalog } from "@dogcodes/protocol";
import { translate } from "./i18n";

type Mode = PermissionCatalog["modes"][number];
type Icon = "ask" | "edit" | "plan" | "full" | "read" | "auto" | "configured";

// IDs belong to each provider: notably Kimi's auto and yolo have different
// meanings from similarly named modes in other agents.
const providerPolicies: Record<string, Record<string, Icon>> = {
  cursor: { default: "ask", "full-access": "full" },
  codex: { "read-only": "read", default: "edit", "full-access": "full" },
  "claude-code": { default: "ask", acceptEdits: "edit", plan: "plan", auto: "auto", dontAsk: "configured", bypassPermissions: "full" },
  grok: { ask: "ask", auto: "auto", "always-approve": "full" },
  zcode: { build: "ask", edit: "edit", plan: "plan", auto: "auto", yolo: "full" },
  kimi: { default: "ask", yolo: "edit", plan: "plan", auto: "full" },
  kiro: { off: "ask", on: "full" },
  opencode: { build: "configured", plan: "plan" },
  antigravity: { configured: "configured", "desktop-approvals": "ask", plan: "plan", "dangerously-skip-permissions": "full" },
};

// Icons group similar policies, but these names would lose meaning under the
// shared label (accepting edits is not "ask when needed").
type Label = Icon | "acceptEdits" | "dontAsk";
const providerLabels: Record<string, Record<string, Label>> = {
  "claude-code": { acceptEdits: "acceptEdits", dontAsk: "dontAsk" },
  zcode: { edit: "acceptEdits" },
};

export function permissionKind(providerId: string, mode?: Mode): Icon | undefined {
  const provider = providerId === "claude" ? "claude-code" : providerId;
  return mode ? providerPolicies[provider]?.[mode.id] : undefined;
}

// Catalogs the core builds itself carry Chinese descriptions; translate those
// known modes and keep descriptions that agents report natively.
const coreDescriptions: Record<string, Record<string, string>> = {
  codex: { "read-only": "codexReadOnly", default: "codexDefault", "full-access": "codexFullAccess" },
  "claude-code": { plan: "claudePlan", default: "claudeDefault", acceptEdits: "claudeAcceptEdits", auto: "claudeAuto", dontAsk: "claudeDontAsk", bypassPermissions: "claudeBypass" },
  antigravity: { "desktop-approvals": "antigravityApprovals", plan: "antigravityPlan", "dangerously-skip-permissions": "antigravitySkip" },
};

// Only normalize known policies. Preserve native text for new ACP modes.
export function permissionPresentation(providerId: string, mode: Mode): Mode {
  const provider = providerId === "claude" ? "claude-code" : providerId;
  const kind = providerLabels[provider]?.[mode.id] ?? permissionKind(providerId, mode);
  const description = coreDescriptions[provider]?.[mode.id];
  return {
    ...mode,
    ...(kind ? { label: translate(`permissionPicker.mode.${kind}`) } : {}),
    ...(description ? { description: translate(`permissionPicker.description.${description}`) } : {}),
  };
}
