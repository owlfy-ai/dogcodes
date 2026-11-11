// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { ApprovalReceipt, BootstrapSnapshot, CoreClient, TaskId, TaskSummary } from "@dogcodes/protocol";
import { ApprovalRecovery } from "./ApprovalRecovery";
import { approvalOperations as operations } from "./approvalOperations";
import { approvalFixture } from "./testFixtures/approval";
import { approvalRecoveryMocks, acceptedApproval } from "./testFixtures/approvalRecovery";
let host: HTMLDivElement, root: Root;
const task: TaskSummary = { taskId: "tsk_test", projectId: "prj_test", title: "Test", streamVersion: 7, activePrimaryRun: null, preferredExecutor: { providerId: "codex", installationId: null } };
beforeEach(() => { localStorage.clear(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
async function fixture() {
  const mocks = approvalRecoveryMocks();
  const respondApproval = vi.fn<CoreClient["respondApproval"]>().mockRejectedValue(new Error("offline"));
  const bootstrap = vi.fn<CoreClient["bootstrap"]>().mockResolvedValue({ tasks: [task] } as BootstrapSnapshot);
  const client = { ...mocks, respondApproval, bootstrap } as unknown as CoreClient;
  await expect(operations.submit(client, task, approvalFixture, "native-once", { cancelled: false, values: { password: "private-answer" } })).rejects.toThrow();
  const record = operations.read(task.taskId)[0];
  const onAccepted = vi.fn();
  const render = (id: TaskId = task.taskId, busy = false, revision = 0) => act(async () => root.render(<ApprovalRecovery client={client} taskId={id} busy={busy} revision={revision} onAccepted={onAccepted}/>));
  return { ...mocks, respondApproval, bootstrap, client, record, onAccepted, render };
}

it("shows nothing and never resends while the original receipt is missing", async () => {
  const f = await fixture(); await f.render();
  expect(host.innerHTML).toBe("");
  expect(f.approvalReceipt).toHaveBeenCalled();
  await act(async () => root.unmount()); root = createRoot(host); await f.render();
  expect(host.innerHTML).toBe("");
  expect(f.respondApproval).toHaveBeenCalledTimes(1);
  expect(operations.read(task.taskId)).toHaveLength(1);
  expect(JSON.stringify(localStorage)).not.toContain("private-answer");
});

it("drops a sent or failed answer and keeps an unknown one locked until it resolves", async () => {
  const f = await fixture();
  f.approvalReceipt.mockResolvedValue(acceptedApproval(f.record.query)); await f.render();
  expect(operations.read(task.taskId)[0]?.accepted).toBe(true);
  expect(f.onAccepted).toHaveBeenCalledOnce();
  f.approvalReceipt.mockResolvedValue(acceptedApproval(f.record.query, "failed")); await f.render(task.taskId, false, 1);
  expect(operations.read(task.taskId)).toEqual([]);
  expect(host.innerHTML).toBe(""); expect(f.respondApproval).toHaveBeenCalledTimes(1);
});

it("does not report results for a conversation that is no longer shown", async () => {
  const f = await fixture(); let resolve!: (value: ApprovalReceipt) => void;
  f.approvalReceipt.mockReturnValueOnce(new Promise(done => { resolve = done; })); await f.render(); await f.render("tsk_other");
  await act(async () => resolve(acceptedApproval(f.record.query, "sent")));
  expect(f.onAccepted).not.toHaveBeenCalled(); expect(host.innerHTML).toBe("");
  expect(operations.read(task.taskId)).toEqual([]);
});

it("waits while an answer is being sent and drops a superseded original", async () => {
  const f = await fixture(); const before = f.approvalReceipt.mock.calls.length;
  await f.render(task.taskId, true);
  expect(f.approvalReceipt).toHaveBeenCalledTimes(before);
  f.bootstrap.mockResolvedValue({ tasks: [{ ...task, streamVersion: 8 }] } as BootstrapSnapshot);
  await f.render();
  expect(operations.read(task.taskId)).toEqual([]); expect(f.onAccepted).toHaveBeenCalledOnce();
  expect(f.respondApproval).toHaveBeenCalledTimes(1); expect(host.innerHTML).toBe("");
});

it("rechecks after a conversation update even when it races an older lookup", async () => {
  const f = await fixture(); let resolve!: (value: ApprovalReceipt) => void;
  f.approvalReceipt.mockReturnValueOnce(new Promise(done => { resolve = done; }));
  await f.render();
  f.approvalReceipt.mockResolvedValue(acceptedApproval(f.record.query, "sent"));
  await f.render(task.taskId, false, 1);
  await act(async () => resolve(acceptedApproval(f.record.query)));
  expect(operations.read(task.taskId)).toEqual([]);
  expect(f.respondApproval).toHaveBeenCalledTimes(1);
});

it("voids an unconfirmed original before sending a different answer", async () => {
  const f = await fixture();
  // Voiding the original is what moves the conversation to version 8.
  const invalidate = vi.fn<NonNullable<CoreClient["invalidateApprovalResponse"]>>().mockImplementation(async () => {
    f.bootstrap.mockResolvedValue({ tasks: [{ ...task, streamVersion: 8 }] } as BootstrapSnapshot);
    return { status: "invalidated" };
  });
  f.client.invalidateApprovalResponse = invalidate;
  await f.render(); expect(invalidate).not.toHaveBeenCalled();
  f.respondApproval.mockResolvedValue(undefined as never);
  await operations.submit(f.client, task, approvalFixture, "native-once", { cancelled: false, values: { password: "a different answer" } });
  expect(invalidate).toHaveBeenCalledExactlyOnceWith(f.record.query, 7);
  expect(f.respondApproval).toHaveBeenCalledTimes(2);
  expect(f.respondApproval.mock.calls[1][0]).toEqual({ taskId: task.taskId, streamVersion: 8 });
});
