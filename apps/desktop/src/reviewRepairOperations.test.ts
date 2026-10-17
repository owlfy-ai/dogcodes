// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, ReviewFixRef, ReviewRepairCall, ReviewRepairEntry } from "@dogcodes/protocol";
import { reviewRepairOperations as operations } from "./reviewRepairOperations";

const reference: ReviewFixRef = { runId: "run_review", inputSha256: "hash", findingIndices: [3, 0, 3] };
function entry(operationId: string, state: "pending" | "submitted"): ReviewRepairEntry {
  const intent = { operationId, taskId: "tsk_test" as const, baseVersion: 7, reference: { ...reference, findingIndices: [0, 3] },
    target: { providerId: "codex", installationId: null }, context: null, createdAt: "now" };
  return state === "pending"
    ? { intent, updatedAt: "now", state, submissionVersion: null, contextConfirmed: false, lastError: "offline" }
    : { intent, updatedAt: "now", state, acknowledgement: { commandId: operationId, accepted: true, firstSequence: 8, lastSequence: 8, requestFingerprint: "v1:hash" } };
}
function fixture() {
  const reviewRepair = vi.fn<(call: ReviewRepairCall) => Promise<ReviewRepairEntry | null>>();
  return { reviewRepair, client: { reviewRepair } as unknown as CoreClient };
}
beforeEach(() => localStorage.clear());

it("sends one normalized start to the service and keeps nothing in local storage", async () => {
  const f = fixture();
  f.reviewRepair.mockImplementation(async call => entry((call as { operationId: string }).operationId, "submitted"));
  const result = await operations.start(f.client, "tsk_test", reference, { providerId: "claude", installationId: "ins" });
  const [call] = f.reviewRepair.mock.calls[0];
  expect(call).toEqual({ action: "start", operationId: expect.stringMatching(/^cmd_/), taskId: "tsk_test",
    reference: { ...reference, findingIndices: [0, 3] }, target: { providerId: "claude", installationId: "ins" } });
  expect(result.state).toBe("submitted");
  expect(localStorage.length).toBe(0);
});

it("treats a lost reply as success only when the service recorded this exact request as submitted", async () => {
  const f = fixture();
  let operationId = "";
  f.reviewRepair.mockImplementation(async call => {
    if (call.action === "start") { operationId = call.operationId; throw new Error("reply lost"); }
    return entry(operationId, "submitted");
  });
  expect((await operations.start(f.client, "tsk_test", reference)).intent.operationId).toBe(operationId);

  f.reviewRepair.mockImplementation(async call => {
    if (call.action === "start") throw new Error("reply lost");
    return entry("cmd_other", "submitted");
  });
  await expect(operations.start(f.client, "tsk_test", reference)).rejects.toThrow("reply lost");

  f.reviewRepair.mockImplementation(async call => {
    if (call.action === "start") { operationId = call.operationId; throw new Error("context changed"); }
    return entry(operationId, "pending");
  });
  await expect(operations.start(f.client, "tsk_test", reference)).rejects.toThrow("context changed");
});

it("continues or abandons with the original identity and rejects invalid selections locally", async () => {
  const f = fixture();
  const pending = entry("cmd_original", "pending");
  f.reviewRepair.mockResolvedValue(entry("cmd_original", "submitted"));
  await operations.resume(f.client, pending);
  await operations.abandon(f.client, pending);
  expect(f.reviewRepair.mock.calls.map(([call]) => call)).toEqual([
    { action: "resume", taskId: "tsk_test", runId: "run_review", operationId: "cmd_original" },
    { action: "abandon", taskId: "tsk_test", runId: "run_review", operationId: "cmd_original" },
  ]);
  await expect(operations.start(f.client, "tsk_test", { ...reference, findingIndices: [] })).rejects.toThrow("有效的审查问题");
  await expect(operations.start({} as CoreClient, "tsk_test", reference)).rejects.toThrow("不支持可靠提交修复");
  expect(f.reviewRepair).toHaveBeenCalledTimes(2);
});
