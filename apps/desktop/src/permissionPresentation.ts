import type { PermissionCatalog } from "@dogcodes/protocol";
import { translate } from "./i18n";

type Mode = PermissionCatalog["modes"][number];
// One kind per meaning; each kind has exactly one icon and one label.
// ask: approve each protected action. onRequest: routine work runs, risky work asks.
// edit: file edits allowed, the rest asks. deny: pre-approved actions only, never asks.
// bypass: approvals skipped, hard limits remain. full: no approvals and no sandbox/guardrails.
export type PermissionKind = "ask" | "onRequest" | "edit" | "plan" | "read" | "auto" | "deny" | "bypass" | "full" | "configured";
type Known = [PermissionKind, description: string];

// IDs belong to each provider: notably Kimi's auto and yolo have different
// meanings from similarly named modes in other agents.
const providerModes: Record<string, Record<string, Known>> = {
  cursor: { default: ["ask", "cursorDefault"], "full-access": ["bypass", "cursorFullAccess"] },
  codex: { "read-only": ["read", "codexReadOnly"], default: ["onRequest", "codexDefault"], "full-access": ["full", "codexFullAccess"] },
  "claude-code": {
    plan: ["plan", "claudePlan"], default: ["ask", "claudeDefault"], acceptEdits: ["edit", "claudeAcceptEdits"],
    auto: ["auto", "claudeAuto"], dontAsk: ["deny", "claudeDontAsk"], bypassPermissions: ["bypass", "claudeBypass"],
  },
  codebuddy: {
    default: ["ask", "codebuddyDefault"], acceptEdits: ["edit", "codebuddyAcceptEdits"], plan: ["plan", "codebuddyPlan"],
    auto: ["auto", "codebuddyAuto"], dontAsk: ["deny", "codebuddyDontAsk"],
    bypassPermissions: ["bypass", "codebuddyBypass"], fullAccess: ["full", "codebuddyFullAccess"],
  },
  grok: { ask: ["ask", "grokAsk"], auto: ["auto", "grokAuto"], "always-approve": ["bypass", "grokAlwaysApprove"] },
  zcode: {
    build: ["ask", "zcodeBuild"], edit: ["edit", "zcodeEdit"], plan: ["plan", "zcodePlan"],
    auto: ["auto", "zcodeAuto"], yolo: ["bypass", "zcodeYolo"],
  },
  kimi: { default: ["ask", "kimiDefault"], yolo: ["onRequest", "kimiYolo"], plan: ["plan", "kimiPlan"], auto: ["bypass", "kimiAuto"] },
  kiro: { off: ["ask", "kiroOff"], on: ["bypass", "kiroOn"] },
  opencode: { build: ["configured", "opencodeBuild"], plan: ["plan", "opencodePlan"] },
  antigravity: {
    "desktop-approvals": ["ask", "antigravityApprovals"], plan: ["plan", "antigravityPlan"],
    "dangerously-skip-permissions": ["bypass", "antigravitySkip"],
  },
  pi: { native: ["full", "piNative"] },
};

// Kinds for modes other ACP agents report natively; their own text is kept.
const nativeKinds: Record<string, PermissionKind> = {
  default: "ask", ask: "ask", acceptEdits: "edit", edit: "edit", plan: "plan", "read-only": "read", readOnly: "read",
  auto: "auto", dontAsk: "deny", build: "configured", configured: "configured",
  bypassPermissions: "bypass", "always-approve": "bypass", "auto-approve": "bypass", "allow-all": "bypass",
  "dangerously-skip-permissions": "bypass", "full-access": "full", fullAccess: "full", "danger-full-access": "full",
};

function known(providerId: string, mode?: Mode): Known | undefined {
  const provider = providerId === "claude" ? "claude-code" : providerId;
  if (!mode) return undefined;
  if (mode.id === "configured") return ["configured", "configured"];
  return providerModes[provider]?.[mode.id];
}

export function permissionKind(providerId: string, mode?: Mode): PermissionKind {
  if (!mode) return "configured";
  return known(providerId, mode)?.[0] ?? nativeKinds[mode.id] ?? (mode.dangerous ? "bypass" : "configured");
}

// Catalogs the core builds carry Chinese text; known modes are translated and
// modes agents report natively keep their own text.
export function permissionPresentation(providerId: string, mode: Mode): Mode {
  const entry = known(providerId, mode);
  if (!entry) return mode;
  const [kind, description] = entry;
  return {
    ...mode,
    label: translate(`permissionPicker.mode.${kind}`),
    description: translate(`permissionPicker.description.${description}`),
  };
}
