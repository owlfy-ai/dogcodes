// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { reviewClient, type ReviewSnapshot } from "./reviewClient";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
beforeEach(() => { vi.resetAllMocks(); localStorage.clear(); });
const query = { taskId: "task", mode: "uncommitted" as const };
const snapshot: ReviewSnapshot = { id: "one", taskId: "task", roots: [] };
it("coalesces pending duplicate reads but performs a fresh query after completion", async () => {
  let finish!: (value: ReviewSnapshot) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const first = reviewClient.query(query);
  const second = reviewClient.query({ ...query, parent: 0, probe: false });
  expect(first).toBe(second);
  expect(invoke).toHaveBeenCalledTimes(1);
  finish(snapshot); await first;
  vi.mocked(invoke).mockResolvedValueOnce({ ...snapshot, id: "fresh" });
  expect((await reviewClient.query(query)).id).toBe("fresh");
  expect(invoke).toHaveBeenCalledTimes(2);
});
it("separates tasks, roots and background probes and releases failed requests", async () => {
  vi.mocked(invoke).mockResolvedValue(snapshot);
  await Promise.all([reviewClient.query(query), reviewClient.query({ ...query, taskId: "other" }), reviewClient.query({ ...query, rootId: "root" }), reviewClient.query({ ...query, probe: true })]);
  expect(invoke).toHaveBeenCalledTimes(4);
  vi.mocked(invoke).mockRejectedValueOnce(new Error("read failed"));
  await expect(reviewClient.query(query)).rejects.toThrow("read failed");
  await expect(reviewClient.query(query)).resolves.toEqual(snapshot);
  expect(invoke).toHaveBeenCalledTimes(6);
});

it("sends both single and batch selections as one index transaction", async () => {
  vi.mocked(invoke).mockResolvedValue(undefined);
  await reviewClient.index("task", "snapshot", "repo", "a", "stage");
  expect(invoke).toHaveBeenLastCalledWith("core_review_index", { operationId: expect.any(String), taskId: "task", snapshotId: "snapshot", rootId: "repo", paths: ["a"], action: "stage" });
  await reviewClient.index("task", "snapshot", "repo", ["a", "b"], "unstage");
  expect(invoke).toHaveBeenLastCalledWith("core_review_index", { operationId: expect.any(String), taskId: "task", snapshotId: "snapshot", rootId: "repo", paths: ["a", "b"], action: "unstage" });
});

it("sends batch file actions with their exact selection and discard confirmation", async () => {
  const result = { completed: ["a"], error: "b changed" }; vi.mocked(invoke).mockResolvedValue(result);
  const request = { taskId: "task", snapshotId: "snapshot", rootId: "repo", paths: ["a", "b"], action: "discard" as const, confirmed: true };
  expect(await reviewClient.batchFiles(request)).toEqual(result);
  expect(invoke).toHaveBeenCalledWith("core_review_batch_files", { request, operationId: expect.any(String) });
});

it("sends the reader identity and ordered lease request unchanged", async () => {
  vi.mocked(invoke).mockResolvedValue(undefined);
  const request = { taskId: "task", snapshotId: "snapshot", leaseId: "reader", revision: 2, release: true };
  await reviewClient.lease(request);
  expect(invoke).toHaveBeenCalledWith("core_review_snapshot_lease", { request });
});
