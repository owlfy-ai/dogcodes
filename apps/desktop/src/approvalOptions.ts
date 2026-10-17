import type { PendingApproval } from "@dogcodes/protocol";

type Option = PendingApproval["options"][number];

export function approvalOptionLabel(option: Option, options: Option[], toolName = ""): string {
  // A role can contain distinct native decisions (e.g. reject once vs always).
  // Keep their native descriptions so the user can choose the intended scope.
  if (options.filter(candidate => candidate.semanticRole === option.semanticRole).length > 1) {
    return option.label;
  }
  switch (option.semanticRole) {
    case "approve_once": return "允许一次";
    case "reject": return "拒绝";
    case "cancel": return "取消本次";
    case "approve_persistent":
      if (/for (?:this |the )?session/i.test(option.label)) {
        return /bash|shell|command|terminal/i.test(toolName) ? "后续允许类似的命令" : "本会话内允许此操作";
      }
      return option.label;
    default: return option.label;
  }
}
