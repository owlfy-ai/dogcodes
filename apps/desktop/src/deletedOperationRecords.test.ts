// @vitest-environment jsdom
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { CoreClient } from "@dogcodes/protocol";
import { retireDeletedOperationRecords, watchDeletedOperationRecords } from "./deletedOperationRecords";
const task = `tsk_${"0".repeat(26)}`, other = `tsk_${"1".repeat(26)}`;
const records: [string, unknown][] = [
  ["dogcodes:pending-git:op", { request: { taskId: task, operationId: "op" } }],
  ["dogcodes:pending-review-mutation:op", { request: { taskId: task, operationId: "op" } }],
  [`dogcodes:pending-fork:${task}:turn`, { taskId: task, turnId: "turn", newTaskId: other }],
  [`dogcodes:pending-review-start:${task}:scope`, { taskId: task, runId: "run" }],
  [`dogcodes:pending-review-repair:${task}:run`, { task: { taskId: task }, operationId: "op", reference: { runId: "run" } }],
  [`dogcodes:pending-cancel:${task}:run`, { task: { taskId: task, activePrimaryRun: { runId: "run" } }, operationId: "op" }],
  [`dogcodes:pending-approval:${task}:turn:1:approve:op`, { query: { taskId: task, turnId: "turn", requestedAtSequence: 1, approvalId: "approve", operationId: "op" } }],
  [`dogcodes.project-run.pending.${task}.op`, { method: "core_project_runs_analyze", request: { taskId: task, operationId: "op" } }],
];
const seed = () => records.forEach(([key, value]) => localStorage.setItem(key, JSON.stringify(value)));
const client = (taskDeletion = vi.fn(async (taskId: string) => ({ taskId, deletionSequence: 4 }))) => ({ taskDeletion } as unknown as CoreClient);
beforeEach(() => localStorage.clear());
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });
it("retires all supported operation domains using one canonical proof and leaves drafts and attachments alone", async () => {
  seed(); localStorage.setItem("my-draft", "keep"); localStorage.setItem("dogcodes:pending-attachment:upload", "keep");
  const core = client(); const changed = vi.fn(); window.addEventListener("storage", changed);
  const result = await retireDeletedOperationRecords(core);
  window.removeEventListener("storage", changed);
  expect(result).toEqual({ retired: 8, issues: [] }); expect(core.taskDeletion).toHaveBeenCalledTimes(1);
  expect(changed).toHaveBeenCalledTimes(8); expect(localStorage.getItem("my-draft")).toBe("keep");
  expect(localStorage.getItem("dogcodes:pending-attachment:upload")).toBe("keep");
});
it.each([null, { taskId: other, deletionSequence: 4 }, { taskId: task, deletionSequence: 0 }])("keeps records without matching positive deletion proof: %s", async proof => {
  seed(); const core = client(vi.fn(async () => proof) as never);
  await retireDeletedOperationRecords(core);
  expect(localStorage.length).toBe(8);
});
it("isolates malformed records and retains bytes replaced while the query is in flight", async () => {
  seed(); localStorage.setItem("dogcodes:pending-git:broken", "{");
  const updated = JSON.stringify({ request: { taskId: other, operationId: "op" } });
  const core = client(vi.fn(async taskId => { localStorage.setItem(records[0][0], updated); return { taskId, deletionSequence: 4 }; }));
  const result = await retireDeletedOperationRecords(core);
  expect(result.retired).toBe(7); expect(result.issues).toHaveLength(1);
  expect(localStorage.getItem(records[0][0])).toBe(updated); expect(localStorage.getItem("dogcodes:pending-git:broken")).toBe("{");
});
it("does not remove anything after disconnect or cancellation", async () => {
  seed();
  await retireDeletedOperationRecords(client(vi.fn(async () => { throw new Error("offline"); }) as never));
  expect(localStorage.length).toBe(8);
  let active = true;
  await retireDeletedOperationRecords(client(vi.fn(async taskId => { active = false; return { taskId, deletionSequence: 4 }; })), () => active);
  expect(localStorage.length).toBe(8);
});
it("coalesces maintenance and prevents a stopped window from applying a late proof", async () => {
  vi.useFakeTimers(); seed(); let resolve!: (proof: { taskId: string; deletionSequence: number }) => void;
  const query = vi.fn(() => new Promise<{ taskId: string; deletionSequence: number }>(done => { resolve = done; }));
  const stop = watchDeletedOperationRecords(client(query as never));
  window.dispatchEvent(new Event("focus")); window.dispatchEvent(new Event("storage"));
  await vi.advanceTimersByTimeAsync(250); expect(query).toHaveBeenCalledTimes(1);
  stop(); resolve({ taskId: task, deletionSequence: 4 }); await Promise.resolve(); await Promise.resolve();
  await vi.advanceTimersByTimeAsync(60000); expect(query).toHaveBeenCalledTimes(1); expect(localStorage.length).toBe(8);
});
