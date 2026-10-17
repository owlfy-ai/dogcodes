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
const button = (label: string) => [...host.querySelectorAll("button")].find(node => node.textContent === label)!;
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
it("restores and queries without submitting or showing a saved answer", async () => {
  const f = await fixture(); await f.render();
  expect(host.textContent).toContain("尚未找到原回执");
  expect(host.textContent).not.toContain("private-answer");
  expect(host.querySelector("textarea,input")).toBeNull();
  await act(async () => root.unmount()); root = createRoot(host); await f.render();
  expect(f.respondApproval).toHaveBeenCalledTimes(1);
  expect(host.textContent).toContain("只重发相同的原请求");
});
it("shows accepted unknown and failed deliveries without a retry decision button", async () => {
  const f = await fixture();
  f.approvalReceipt.mockResolvedValue(acceptedApproval(f.record.query)); await f.render();
  expect(host.textContent).toContain("已受理不代表任务已完成");
  f.approvalReceipt.mockResolvedValue(acceptedApproval(f.record.query, "failed"));
  await act(async () => button("核实审批结果").click());
  expect(host.textContent).toContain("驱动报告投递失败"); expect(f.respondApproval).toHaveBeenCalledTimes(1);
  await act(async () => button("已核对，结束审批跟踪").click());
  expect(operations.read(task.taskId)).toEqual([]);
});
it("settles sent receipts even after the row is removed, and suppresses stale-task callbacks", async () => {
  const f = await fixture();
  f.approvalReceipt.mockResolvedValue(acceptedApproval(f.record.query, "sent")); await f.render();
  expect(host.querySelector('[aria-label="审批结果待核实"]')).toBeNull(); expect(f.onAccepted).toHaveBeenCalledOnce();
  await act(async () => root.unmount()); root = createRoot(host);
  const g = await fixture(); let resolve!: (value: ApprovalReceipt) => void;
  g.approvalReceipt.mockReturnValueOnce(new Promise(done => { resolve = done; })); await g.render(); await g.render("tsk_other");
  await act(async () => resolve(acceptedApproval(g.record.query, "sent")));
  expect(g.onAccepted).not.toHaveBeenCalled(); expect(host.textContent).toBe("");
});
it("waits for the initial submission and explains a verified obsolete version", async () => {
  const f = await fixture(); const before = f.approvalReceipt.mock.calls.length;
  await f.render(task.taskId, true); expect(button("核实审批结果").disabled).toBe(true);
  expect(f.approvalReceipt).toHaveBeenCalledTimes(before);
  f.bootstrap.mockResolvedValue({ tasks: [{ ...task, streamVersion: 8 }] } as BootstrapSnapshot);
  await f.render(); expect(host.textContent).toContain("请重新查看请求，再确认提交");
  expect(f.respondApproval).toHaveBeenCalledTimes(1); expect(operations.read(task.taskId)).toEqual([]);
});

it("invalidates only on an explicit change decision action, never while restoring", async () => {
  const f = await fixture();
  const invalidate = vi.fn<NonNullable<CoreClient["invalidateApprovalResponse"]>>().mockResolvedValue({ status: "invalidated" });
  f.client.invalidateApprovalResponse = invalidate;
  await f.render(); expect(invalidate).not.toHaveBeenCalled();
  await act(async () => button("修改未受理的决定").click());
  expect(invalidate).toHaveBeenCalledExactlyOnceWith(f.record.query, 7);
  expect(host.textContent).toContain("请重新查看请求，再确认提交");
  expect(f.respondApproval).toHaveBeenCalledTimes(1);
});

it("refreshes delivery after a task event even when it races an older lookup", async () => {
  const f = await fixture(); let resolve!: (value: ApprovalReceipt) => void;
  f.approvalReceipt.mockReturnValueOnce(new Promise(done => { resolve = done; }));
  await f.render();
  f.approvalReceipt.mockResolvedValue(acceptedApproval(f.record.query, "sent"));
  await f.render(task.taskId, false, 1);
  await act(async () => resolve(acceptedApproval(f.record.query)));
  expect(operations.read(task.taskId)).toEqual([]);
  expect(host.querySelector('[aria-label="审批结果待核实"]')).toBeNull();
  expect(f.respondApproval).toHaveBeenCalledTimes(1);
});
