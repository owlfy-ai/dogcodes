import { createHash } from "node:crypto";
import { vi } from "vitest";
import type { ApprovalReceipt, CoreClient } from "@dogcodes/protocol";
/** Test-only host hashing; production uses the Rust typed-command fingerprint. */
export function approvalRecoveryMocks() {
  const prepareApprovalResponse = vi.fn<NonNullable<CoreClient["prepareApprovalResponse"]>>(async (task, approvalId, optionId, source, answer, operationId) => ({
    taskId: task.taskId, ...source, approvalId, operationId,
    requestFingerprint: `v1:${createHash("sha256").update(JSON.stringify({task, approvalId, optionId, source, answer})).digest("hex")}`,
  }));
  const approvalReceipt = vi.fn<NonNullable<CoreClient["approvalReceipt"]>>().mockResolvedValue(null);
  return { prepareApprovalResponse, approvalReceipt };
}
export function acceptedApproval(query: Parameters<NonNullable<CoreClient["approvalReceipt"]>>[0], delivery: ApprovalReceipt["delivery"] = "unknown"): ApprovalReceipt {
  return { acknowledgement: { accepted: true, commandId: query.operationId, requestFingerprint: query.requestFingerprint, firstSequence: 20, lastSequence: 20 }, delivery };
}
