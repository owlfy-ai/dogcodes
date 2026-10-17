// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, ReviewRepairCall, ReviewRepairEntry, TaskSummary, TurnReviewRun } from "@dogcodes/protocol";
import { ReviewRepairControls } from "./ReviewRepairControls";

const task: TaskSummary = { taskId: "tsk_test", projectId: "prj_test", title: "Task", streamVersion: 7,
  preferredExecutor: { providerId: "codex", installationId: null }, activePrimaryRun: null };
const run: TurnReviewRun = { request: { taskId: task.taskId, turnId: null, sourceSnapshot: "snapshot", runId: "run_review", kind: "code", executor: { providerId: "kimi", installationId: null }, retryOf: null },
  status: "completed", createdAt: "2026-10-16T00:00:00Z", finishedAt: null, nativeSessionId: null, sourceSequence: 1, sourceArtifact: null,
  inputSha256: "hash", coverage: "", result: "", error: null };
let element: HTMLDivElement, root: Root;
const button = (label: string) => [...element.querySelectorAll("button")].find(node => node.textContent === label)!;
const click = async (label: string) => act(async () => button(label).click());
const panel = () => element.querySelector('[aria-label="待核实的修复请求"]');
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); element = document.createElement("div"); document.body.append(element); root = createRoot(element); });
afterEach(async () => { await act(async () => root.unmount()); element.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function pending(operationId: string, findingIndices = [0], extra: Partial<Extract<ReviewRepairEntry, { state: "pending" }>> = {}): ReviewRepairEntry {
  return { intent: { operationId, taskId: task.taskId, baseVersion: 7, reference: { runId: run.request.runId, inputSha256: "hash", findingIndices },
    target: { providerId: "claude", installationId: null }, context: null, createdAt: "now" },
  updatedAt: "now", state: "pending", submissionVersion: null, contextConfirmed: false, lastError: null, ...extra };
}
const submitted = (original: ReviewRepairEntry): ReviewRepairEntry => ({ intent: original.intent, updatedAt: "later", state: "submitted",
  acknowledgement: { commandId: original.intent.operationId, accepted: true, firstSequence: 8, lastSequence: 8, requestFingerprint: "v1:hash" } });

function fixture(initial: ReviewRepairEntry | null) {
  let record = initial;
  const reviewRepair = vi.fn(async (call: ReviewRepairCall): Promise<ReviewRepairEntry | null> => {
    if (call.action === "get") return record;
    if (call.action === "resume" && record) { record = submitted(record); return record; }
    if (call.action === "abandon" && record) { record = { intent: record.intent, updatedAt: "later", state: "abandoned" }; return record; }
    throw new Error("unexpected");
  });
  const clearSelection = vi.fn();
  const client = { bootstrap: vi.fn(async () => ({ tasks: [task] })), turnReviews: vi.fn(async () => [run]), reviewRepair } as unknown as CoreClient;
  const render = (selected = [0]) => act(async () => root.render(
    <ReviewRepairControls client={client} run={run} selected={selected} clearSelection={clearSelection} renderReport={() => null}/>));
  return { reviewRepair, clearSelection, render, set: (value: ReviewRepairEntry | null) => { record = value; } };
}

it("shows a repair recorded by the service, including one started on another client, and blocks a new start", async () => {
  const f = fixture(pending("cmd_remote", [0, 2], { contextConfirmed: true, lastError: "会话已变化" }));
  await f.render([1]);
  expect(panel()?.textContent).toContain("问题 1、问题 3 · ");
  expect(panel()?.textContent).toContain("Agent 已切换");
  expect(panel()?.textContent).toContain("会话已变化");
  expect(button("开始修复").disabled).toBe(true);
  expect(f.reviewRepair.mock.calls.every(([call]) => call.action === "get")).toBe(true);
});

it("continues the original request once and clears the selection only after the service submitted it", async () => {
  const original = pending("cmd_original");
  const f = fixture(original);
  await f.render();
  await click("继续原修复");
  expect(f.reviewRepair).toHaveBeenCalledWith({ action: "resume", taskId: task.taskId, runId: run.request.runId, operationId: "cmd_original" });
  expect(f.clearSelection).toHaveBeenCalledOnce();
  expect(panel()).toBeNull();
  expect(button("开始修复").disabled).toBe(false);
});

it("abandons without clearing the selection and lets a new repair start afterwards", async () => {
  const f = fixture(pending("cmd_original"));
  await f.render();
  await click("放弃这次修复");
  expect(f.reviewRepair).toHaveBeenCalledWith({ action: "abandon", taskId: task.taskId, runId: run.request.runId, operationId: "cmd_original" });
  expect(f.clearSelection).not.toHaveBeenCalled();
  expect(panel()).toBeNull();
  expect(button("开始修复").disabled).toBe(false);
});

it("does not clear a newer selection when an older request's reply arrives late", async () => {
  const original = pending("cmd_original");
  const f = fixture(original);
  let resolve!: (value: ReviewRepairEntry) => void;
  f.reviewRepair.mockImplementationOnce(async () => original)
    .mockImplementationOnce(() => new Promise(done => { resolve = done; }));
  await f.render();
  await act(async () => { button("继续原修复").click(); });
  // Another client replaced the record while this reply was in flight.
  const newer = pending("cmd_newer", [2]);
  f.set(newer);
  await f.render([2]);
  await act(async () => resolve(submitted(newer)));
  expect(f.clearSelection).not.toHaveBeenCalled();
});
