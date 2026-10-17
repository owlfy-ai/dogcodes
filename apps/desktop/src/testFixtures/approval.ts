import type { PendingApproval } from "@dogcodes/protocol";

export const approvalFixture: PendingApproval = {
  approvalId: "approval_fixture",
  turnId: "trn_one",
  toolName: "运行终端命令",
  commandSummary: "pnpm --filter @dogcodes/desktop test",
  requestedAtSequence: 18,
  options: [
    { optionId: "native-deny", label: "拒绝", semanticRole: "reject" },
    { optionId: "native-once", label: "允许一次", semanticRole: "approve_once" },
  ],
};
