import { expect, it } from "vitest";
import { approvalOptionLabel } from "./approvalOptions";
it("preserves the native scope of persistent approval", () => {
  const session = { optionId: "approve_always", label: "Approve for this session", semanticRole: "approve_persistent" as const };
  expect(approvalOptionLabel(session, [session])).toBe("本会话内允许此操作");
  expect(approvalOptionLabel(session, [session], "Bash")).toBe("后续允许类似的命令");
  const scoped = { ...session, label: "Allow git status commands" };
  expect(approvalOptionLabel(scoped, [scoped])).toBe(scoped.label);
});
