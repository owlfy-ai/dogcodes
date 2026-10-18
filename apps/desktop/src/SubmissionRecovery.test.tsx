// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { DemoCoreClient } from "./demoClient";
import { PendingSubmissionError, submissionOperations as operations } from "./submissionOperations";
import { SubmissionRecovery } from "./SubmissionRecovery";
let root: Root, host: HTMLDivElement;
beforeEach(() => { localStorage.clear(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const click = async (label: string) => act(async () => { [...host.querySelectorAll("button")].find(button => button.textContent === label)!.click(); });
const panel = () => host.querySelector('[aria-label="待核实的发送请求"]');
/** A new-conversation send the service stored but could not finish (creation failed). */
async function fixture() {
  const client = new DemoCoreClient(), task = (await client.bootstrap()).tasks[0];
  const create = vi.spyOn(client, "createTask").mockRejectedValueOnce(new Error("offline")), submit = vi.spyOn(client, "submitTurn");
  const failure = await operations.start(client, { scope: "draft:test", taskId: null, projectId: task.projectId, target: task.preferredExecutor,
    input: { prompt: "original", model: "model", thinking: "high", permissionMode: null, attachments: [] } }).catch(cause => cause);
  expect(failure).toBeInstanceOf(PendingSubmissionError);
  const record = (failure as PendingSubmissionError).intent;
  const onResolved = vi.fn();
  const render = (allowed = true, busy = false) => act(async () => root.render(<SubmissionRecovery client={client} busy={busy} canResume={() => allowed} onResolved={onResolved}/>));
  return { client, record, onResolved, create, submit, render };
}

it("shows the service's pending original and continues it once with the frozen identities", async () => {
  const f = await fixture(); await f.render();
  expect(panel()?.textContent).toContain("original"); expect(host.textContent).toContain("model"); expect(host.textContent).toContain("high");
  await act(async () => root.unmount()); root = createRoot(host); await f.render();
  expect(f.create).toHaveBeenCalledTimes(1); expect(f.submit).not.toHaveBeenCalled();
  await click("核实发送结果");
  expect(f.create).toHaveBeenCalledTimes(1);
  expect(host.textContent).toContain("原消息尚未受理");
  await act(async () => { [...host.querySelectorAll("button")].find(button => button.textContent === "继续原发送")!.click(); await vi.waitFor(() => expect(f.onResolved).toHaveBeenCalled(), { timeout: 3000 }); });
  expect(f.create).toHaveBeenLastCalledWith(f.record.task.projectId, "新对话", f.record.target, { taskId: f.record.task.taskId, operationId: f.record.creation!.operationId });
  expect(f.submit).toHaveBeenCalledTimes(1);
  expect(f.onResolved).toHaveBeenCalledWith(f.record, expect.objectContaining({ status: "submitted", taskId: f.record.task.taskId }));
});

it("allows checking and safely abandoning a request while its Agent is unavailable", async () => {
  const f = await fixture(); await f.render(false); await click("核实发送结果");
  expect([...host.querySelectorAll("button")].find(button => button.textContent === "继续原发送")?.disabled).toBe(true);
  await click("放弃原发送");
  expect(f.onResolved).toHaveBeenCalledWith(f.record, { status: "invalidated", taskId: f.record.task.taskId });
  expect(host.textContent).toContain("已放弃尚未受理的原发送");
  expect(panel()).toBeNull();
  expect(f.create).toHaveBeenCalledTimes(1); expect(f.submit).not.toHaveBeenCalled();
});

it("does not show uncertain-result warnings during a normal initial send", async () => {
  const f = await fixture(); await f.render(true, true);
  expect(panel()).toBeNull();
  await f.render();
  expect(panel()).not.toBeNull();
});

it("shows permanent deletion separately and records the proven ending in the service", async () => {
  const f = await fixture();
  const resolution = { status: "task_deleted" as const, taskId: f.record.task.taskId, deletionSequence: 99 };
  const close = vi.fn(async () => ({ state: "closed" as const, fingerprint: "bound", resolution }));
  Object.assign(f.client, { submissionIntentResolve: vi.fn(async () => resolution), submissionIntentClose: close });
  await f.render(false); await click("核实发送结果");
  expect(close).toHaveBeenCalledWith(f.record);
  expect(f.onResolved).toHaveBeenCalledWith(f.record, resolution);
  expect(host.textContent).toContain("原会话已永久删除，发送恢复记录已结束");
  expect(host.textContent).not.toContain("已放弃尚未受理");
  expect(f.submit).not.toHaveBeenCalled();
});

it("reports a discovery failure without dropping records already listed", async () => {
  const f = await fixture(); await f.render();
  expect(panel()).not.toBeNull();
  vi.spyOn(f.client, "submissionIntentList").mockRejectedValue(new Error("offline"));
  await act(async () => { await operations.discover(f.client); });
  expect(host.textContent).toContain("暂时无法同步后台发送记录");
  expect(panel()).not.toBeNull();
});

it("keeps a rejected recovery retry visible with its message and attachments", async () => {
  const client = new DemoCoreClient(), task = (await client.bootstrap()).tasks[0];
  const start = vi.spyOn(client, "submissionStart").mockRejectedValueOnce(new Error("reply lost"));
  const lookup = vi.spyOn(client, "submissionIntentGet").mockRejectedValueOnce(new Error("offline"));
  await operations.start(client, { scope: "draft:recover", taskId: null, projectId: task.projectId, target: task.preferredExecutor,
    input: { prompt: "keep original", model: null, thinking: null, permissionMode: null,
      attachments: [{ name: "notes.txt", path: "/app/notes.txt", mimeType: "text/plain" }] } }).catch(() => {});
  const onRestore = vi.fn();
  await act(async () => root.render(<SubmissionRecovery client={client} busy={false} canResume={() => true} onResolved={vi.fn()} onRestore={onRestore}/>));
  lookup.mockResolvedValue(null);
  await click("核实发送结果");
  start.mockRejectedValueOnce(new Error("原会话不可发送，请等待回复结束或刷新后重试。"));
  await click("重新发送原内容");
  const unknown = host.querySelector('[aria-label="结果未知的发送请求"]');
  expect(unknown?.textContent).toContain("keep original");
  expect(unknown?.textContent).toContain("notes.txt");
  expect(unknown?.textContent).toContain("原会话不可发送");
  expect(start.mock.calls[1][0].operationId).toBe(start.mock.calls[0][0].operationId);
  expect(onRestore).not.toHaveBeenCalled();
  await click("放弃并放回输入框");
  expect(onRestore).toHaveBeenCalledWith(expect.objectContaining({ input: expect.objectContaining({ prompt: "keep original" }) }));
  expect(host.querySelector('[aria-label="结果未知的发送请求"]')).toBeNull();
});
