// @vitest-environment jsdom
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { CoreClient, ProjectReviewRequest, TurnReviewRun } from "@dogcodes/protocol";
import { reviewStartOperations as operations } from "./reviewStartOperations";
const request: ProjectReviewRequest = { taskId: "tsk_test", turnId: null, runId: "run_original", kind: "code", executor: { providerId: "codex", installationId: "ready" }, retryOf: null, sourceSnapshot: "snapshot" };
const result = (value = request): TurnReviewRun => ({ request: value, status: "running", createdAt: "now", finishedAt: null, nativeSessionId: null, sourceSequence: 1, sourceArtifact: null, inputSha256: "hash", coverage: "", result: "", error: null });
const startProjectReview = vi.fn(), startTurnReview = vi.fn(), turnReviews = vi.fn();
const client = { startProjectReview, startTurnReview, turnReviews } as unknown as CoreClient;
beforeEach(() => { localStorage.clear(); vi.resetAllMocks(); startProjectReview.mockRejectedValue(new Error("offline")); turnReviews.mockResolvedValue([]); });
afterEach(() => vi.restoreAllMocks());
it("does not dispatch when saving the original request fails", async () => {
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage full"); });
  await expect(operations.send(client, "project", request)).rejects.toThrow("storage full");
  expect(startProjectReview).not.toHaveBeenCalled();
});
it("resolves lost replies from a fully matching persisted run", async () => {
  turnReviews.mockResolvedValue([result()]);
  expect(await operations.send(client, "project", request)).toEqual(result());
  expect(startProjectReview).toHaveBeenCalledTimes(1);
  expect(operations.read(request.taskId, "project")).toBeNull();
});
it.each([
  { executor: { providerId: "kimi", installationId: null } }, { sourceSnapshot: "other" }, { retryOf: "run_other" },
  { kind: "security" as const }, { recheckOf: { runId: "run_other", repairTurnId: "trn_fix" } },
])("rejects conflicting receipt fields without forgetting or resending: %j", async change => {
  await expect(operations.send(client, "project", request)).rejects.toThrow();
  turnReviews.mockResolvedValue([result({ ...request, ...change })]);
  await expect(operations.resend(client, "project", request)).rejects.toThrow("不匹配");
  expect(startProjectReview).toHaveBeenCalledTimes(1); expect(operations.read(request.taskId, "project")).toEqual(request);
});
it("rejects new payloads while the original outcome remains unknown", async () => {
  await expect(operations.send(client, "project", request)).rejects.toThrow();
  await expect(operations.send(client, "project", { ...request, runId: "run_new" })).rejects.toThrow("先核实");
  expect(startProjectReview).toHaveBeenCalledTimes(1);
});
it("keeps a newer pending operation when an old lookup finishes late", async () => {
  await expect(operations.send(client, "project", request)).rejects.toThrow();
  let resolve!: (value: TurnReviewRun[]) => void;
  turnReviews.mockReturnValueOnce(new Promise(done => { resolve = done; }));
  const pending = operations.check(client, "project", request);
  operations.forget("project", request);
  const newer = { ...request, runId: "run_new" };
  await expect(operations.send(client, "project", newer)).rejects.toThrow();
  resolve([result()]); await pending;
  expect(operations.read(request.taskId, "project")).toEqual(newer);
});
it("accepts omitted optional fields as equivalent to null", async () => {
  const optional = { ...request, executor: { providerId: "codex", installationId: null }, recheckOf: null };
  const persisted = JSON.parse(JSON.stringify(result({ ...request, executor: { providerId: "codex", installationId: null } })));
  delete persisted.request.executor.installationId;
  turnReviews.mockResolvedValue([persisted]);
  await operations.send(client, "project", optional);
  expect(operations.read(request.taskId, "project")).toBeNull();
});
