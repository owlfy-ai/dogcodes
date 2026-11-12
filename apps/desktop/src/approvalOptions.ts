import type { PendingApproval } from "@dogcodes/protocol";
import { displayBackendText } from "./backendText";
import { translate as t } from "./i18n";

type Option = PendingApproval["options"][number];

export function approvalOptionLabel(option: Option, options: Option[], toolName = ""): string {
  // A role can contain distinct native decisions (e.g. reject once vs always).
  // Keep their native descriptions so the user can choose the intended scope.
  if (options.filter(candidate => candidate.semanticRole === option.semanticRole).length > 1) {
    return displayBackendText(option.label);
  }
  switch (option.semanticRole) {
    case "approve_once": return t("approvalOptions.approveOnce");
    case "reject": return t("approvalOptions.reject");
    case "cancel": return t("approvalOptions.cancel");
    case "approve_persistent":
      if (/for (?:this |the )?session/i.test(option.label)) {
        return t(/bash|shell|command|terminal/i.test(toolName) ? "approvalOptions.allowSimilar" : "approvalOptions.allowSession");
      }
      return displayBackendText(option.label);
    default: return displayBackendText(option.label);
  }
}
