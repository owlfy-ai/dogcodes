// @vitest-environment jsdom
import { approvalRecoveryMocks } from "./testFixtures/approvalRecovery";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";
import { approvalFixture } from "./testFixtures/approval";
import type { PendingApproval, EventStreamMessage } from "@dogcodes/protocol";
let root: Root, element: HTMLDivElement, client: DemoCoreClient;
let sequence = 6;
let pending: PendingApproval[], notify: (message: EventStreamMessage) => void;
const click = async (target: Element) => act(async () => { (target as HTMLElement).click(); });
const sync = async () => { await act(async () => { notify({ type: "event", data: { schemaVersion: 1, sequence: ++sequence, eventId: `evt_${sequence}`, stream: { streamType: "system", streamId: "test" }, streamVersion: sequence, eventType: "approval.resolved", occurredAt: new Date().toISOString(), recordedAt: new Date().toISOString(), correlationId: "cor_test", causationId: null, source: { kind: "hub" }, payload: {}, vendorExtensions: null } }); await new Promise(resolve => setTimeout(resolve, 70)); }); };
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
  client = new DemoCoreClient(); localStorage.clear(); Object.assign(client, approvalRecoveryMocks()); pending = []; sequence = (await client.bootstrap()).lastSequence;
  vi.spyOn(client, "pendingApprovals").mockImplementation(async () => [...pending]);
  vi.spyOn(client, "subscribe").mockImplementation(async (_, listener) => { notify = listener; return () => {}; });
  element = document.createElement("div"); document.body.append(element); root = createRoot(element);
  await act(async () => root.render(<App client={client} />));
});
afterEach(async () => { await act(async () => root.unmount()); element.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it("replaces composer until all approvals finish and preserves draft", async () => {
  await act(async () => {
    const input = element.querySelector("textarea")!;
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(input, "保留草稿");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  pending = [approvalFixture, { ...approvalFixture, approvalId: "second" }]; await sync();
  expect(element.querySelector("textarea")).toBeNull();
  expect(element.querySelectorAll(".approval-card")).toHaveLength(2);
  let finish!: () => void;
  const respond = vi.spyOn(client, "respondApproval").mockImplementation(async (_, id) => {
    await new Promise<void>(resolve => { finish = resolve; }); pending = pending.filter(a => a.approvalId !== id);
  });
  const allow = element.querySelector(".approval-primary")!;
  await click(allow); await click(allow);
  expect(respond).toHaveBeenCalledTimes(1);
  expect(element.querySelector("textarea")).toBeNull();
  await act(async () => finish());
  expect(element.querySelectorAll(".approval-card")).toHaveLength(1);
  expect(element.querySelector("textarea")).toBeNull();
  pending = []; await sync(); // The remaining approval was resolved by the agent.
  expect(element.querySelector("textarea")?.value).toBe("保留草稿");
});
it("keeps failed approvals visible and preserves native dropdown decisions", async () => {
  pending = [{ ...approvalFixture, commandSummary: '{"command":"echo hello","requestId":"internal-id"}', options: [...approvalFixture.options, { optionId: "native-always", label: "Allow always", semanticRole: "approve_persistent" }] }];
  await sync();
  expect(element.textContent).toContain("echo hello"); expect(element.textContent).not.toContain("internal-id");
  const respond = vi.spyOn(client, "respondApproval").mockRejectedValue(new Error("审批提交失败"));
  await click(element.querySelector(".approval-more")!); await click(element.querySelector(".approval-menu button")!);
  expect(respond).toHaveBeenCalledWith(expect.anything(), approvalFixture.approvalId, "native-always", { turnId: approvalFixture.turnId, requestedAtSequence: approvalFixture.requestedAtSequence }, undefined, expect.stringMatching(/^cmd_/));
  expect(element.textContent).toContain("审批提交失败"); expect(element.querySelector("textarea")).toBeNull();
  expect(element.querySelector<HTMLButtonElement>(".approval-primary")?.disabled).toBe(false);
});

it("applies full access to the active Cursor run while an approval is displayed", async () => {
  const snapshot=await client.bootstrap();
  const executor={providerId:"cursor",installationId:"ins_cursor"} as const;
  const task=snapshot.tasks[0];
  task.preferredExecutor=executor;
  task.activePrimaryRun={runId:"run_cursor",turnId:"trn_cursor",executor,state:"waiting_for_approval",startedAt:""};
  snapshot.installations=[{...snapshot.installations[0],...executor}];
  pending=[{...approvalFixture,turnId:"trn_cursor"}];
  vi.spyOn(client,"bootstrap").mockResolvedValue(snapshot);
  let permissionMode="default";
  const update=vi.fn(async()=>{pending=[];permissionMode="full-access";});
  Object.assign(client,{
    setRunningPermissionMode:update,
    permissionCatalog:vi.fn(async()=>({defaultModeId:permissionMode,modes:[
      {id:"default",label:"操作前确认",description:"",dangerous:false},
      {id:"full-access",label:"完全访问",description:"",dangerous:true},
    ]})),
  });
  await act(async()=>root.render(<App key="cursor" client={client}/>));
  expect(element.querySelectorAll(".approval-card")).toHaveLength(1);
  await click(element.querySelector('[aria-label="权限策略"]')!);
  await click(element.querySelectorAll('[role="menuitemradio"]')[1]);
  expect(update).toHaveBeenCalledExactlyOnceWith(task.taskId,"run_cursor",executor,"full-access");
  await sync();
  expect(element.querySelector(".approval-card")).toBeNull();
  expect(element.querySelector('[aria-label="权限策略"]')?.textContent).toContain("完全访问");
});

it("keeps answers transient and resets a form when a native ID is reused", async () => {
  pending = [{ ...approvalFixture, question: { title: "Question", schema: { type: "object", required: ["password"], properties: { password: { type: "string", title: "Password", writeOnly: true } } } } }];
  await sync();
  const enter = async () => act(async () => {
    const input = element.querySelector<HTMLInputElement>('input[type="password"]')!;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "private-answer");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await enter();
  const respond = vi.spyOn(client, "respondApproval").mockRejectedValue(new Error("offline"));
  await act(async () => element.querySelector("form.question-card")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
  expect(respond).toHaveBeenCalledOnce();
  expect(element.querySelector<HTMLInputElement>('input[type="password"]')!.value).toBe("private-answer");
  expect(JSON.stringify(localStorage)).not.toContain("private-answer");
  pending = [{ ...pending[0], requestedAtSequence: pending[0].requestedAtSequence + 1 }];
  await sync();
  expect(element.querySelector<HTMLInputElement>('input[type="password"]')!.value).toBe("");
  await enter();
  await act(async () => root.unmount()); root = createRoot(element);
  await act(async () => root.render(<App client={client}/>));
  expect(element.querySelector<HTMLInputElement>('input[type="password"]')!.value).toBe("");
  expect(respond).toHaveBeenCalledOnce();
});

it("clears only the recovered approval error when a lost receipt is found", async () => {
  pending = [approvalFixture]; await sync();
  const respond = vi.spyOn(client, "respondApproval").mockRejectedValue(new Error("本条审批暂未收到回执"));
  await click(element.querySelector(".approval-primary")!);
  expect(element.textContent).toContain("本条审批暂未收到回执");
  const adapter = client as import("@dogcodes/protocol").CoreClient;
  vi.mocked(adapter.approvalReceipt!).mockImplementation(async query => ({ acknowledgement: { accepted: true, commandId: query.operationId, requestFingerprint: query.requestFingerprint, firstSequence: 20, lastSequence: 20 }, delivery: "sent" }));
  pending = [];
  await click([...element.querySelectorAll("button")].find(node => node.textContent === "核实审批结果")!);
  expect(element.textContent).not.toContain("本条审批暂未收到回执");
  expect(respond).toHaveBeenCalledOnce();
});
