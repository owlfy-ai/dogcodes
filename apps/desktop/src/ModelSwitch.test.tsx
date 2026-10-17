// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";

let root: Root, element: HTMLDivElement, client: DemoCoreClient;
const button = (label: string) => [...element.querySelectorAll("button")].find(b => b.textContent?.includes(label))!;
const click = async (target: Element) => act(async () => { (target as HTMLElement).click(); });
const typePrompt = async (text: string) => act(async () => {
  const input = element.querySelector("textarea")!;
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(input, text);
  input.dispatchEvent(new Event("input", { bubbles: true }));
});
const send = async () => act(async () => { element.querySelector("form.composer")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
let snapshot: Awaited<ReturnType<DemoCoreClient["bootstrap"]>>;
let notify: (message: import("@dogcodes/protocol").EventStreamMessage) => void;
const sync = async () => { await act(async () => { notify({ type: "event", data: { schemaVersion: 1, sequence: ++snapshot.lastSequence, eventId: `evt_${snapshot.lastSequence}`, stream: { streamType: "system", streamId: "test" }, streamVersion: snapshot.lastSequence, eventType: "turn.completed", occurredAt: new Date().toISOString(), recordedAt: new Date().toISOString(), correlationId: "cor_test", causationId: null, source: { kind: "hub" }, payload: {}, vendorExtensions: null } }); await new Promise(resolve => setTimeout(resolve, 70)); }); };
beforeEach(async () => {
  localStorage.clear();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
  client = new DemoCoreClient();
  snapshot = await client.bootstrap();
  const task = snapshot.tasks[0];
  snapshot.sessionMaps = [{ taskId: task.taskId, executor: task.preferredExecutor, sessionMapId: "ses_existing", nativeSessionId: "existing", model: "old", thinking: "high", lastNativeTurnId: null, workspaceLocator: "/tmp", driverVersion: null, nativeAgentVersion: null, createdAt: "", lastUsedAt: "", resumeCapability: "supported", status: "active", lastSeenSequence: 0 }];
  Object.assign(client, { availableModels: vi.fn().mockResolvedValue({ defaultModel: "old", models: [
    { id: "old", label: "Old model", thinkingOptions: [{ id: "high", label: "High" }, { id: "low", label: "Low" }], defaultThinking: "high" },
    { id: "new", label: "New model", thinkingOptions: [], defaultThinking: null },
  ] }) });
  vi.spyOn(client, "bootstrap").mockImplementation(async () => structuredClone(snapshot));
  vi.spyOn(client, "subscribe").mockImplementation(async (_, listener) => { notify = listener; return () => {}; });
  const accepted = new Map<string, { signature: string; receipt: import("@dogcodes/protocol").CommandReceipt }>();
  const events: import("@dogcodes/protocol").EventEnvelope[] = [];
  const signature = (task: import("@dogcodes/protocol").TaskSummary, input: import("@dogcodes/protocol").TurnSubmission) => JSON.stringify([task.taskId, task.streamVersion, task.preferredExecutor, input]);
  vi.spyOn(client, "submitTurnReceipt").mockImplementation(async (task, input, id) => {
    const found = accepted.get(id);
    if (found && found.signature !== signature(task, input)) throw new Error("different original input");
    return found?.receipt ?? null;
  });
  vi.spyOn(client, "syncEvents").mockImplementation(async (after, limit = 100) => events.filter(event => event.sequence > after).slice(0, limit));
  vi.spyOn(client, "submitTurn").mockImplementation(async (task, prompt, model = null, thinking = null, permissionMode = null, attachments = [], id) => {
    if (!id) throw new Error("fixed identity required");
    const current = snapshot.tasks.find(item => item.taskId === task.taskId)!;
    if (current.streamVersion !== task.streamVersion) throw new Error("stale version");
    const sequence = ++snapshot.lastSequence;
    accepted.set(id, { signature: signature(task, { prompt, model, thinking, permissionMode, attachments }),
      receipt: { commandId: id, accepted: true, firstSequence: sequence, lastSequence: sequence, requestFingerprint: "test:bound-input" } });
    const event: import("@dogcodes/protocol").EventEnvelope = { schemaVersion: 1, sequence, eventId: `evt_${sequence}`,
      stream: { streamType: "task", streamId: task.taskId }, streamVersion: ++current.streamVersion, eventType: "turn.queued",
      occurredAt: new Date().toISOString(), recordedAt: new Date().toISOString(), correlationId: "cor_test", causationId: id,
      source: { kind: "hub" }, payload: { taskId: task.taskId, turnId: `trn_${sequence}`, prompt, executor: task.preferredExecutor }, vendorExtensions: null };
    events.push(event); notify({ type: "event", data: event });
  });
  element = document.createElement("div"); document.body.append(element); root = createRoot(element);
  await act(async () => root.render(<App client={client} />));
});
afterEach(async () => { await act(async () => root.unmount()); element.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it("changes a saved session's model for the next turn and can reset to native defaults", async () => {
  await click(element.querySelector('[aria-label="选择模型"]')!);
  await click(button("New model"));
  expect(element.querySelector('[aria-label="选择思考强度"]')).toBeNull();
  await sync(); // A refresh with the old mapping must not overwrite the choice.
  await typePrompt("use new model"); await send();
  expect(client.submitTurn).toHaveBeenLastCalledWith(expect.anything(), "use new model", "new", null, null, [], expect.any(String));
  await click(element.querySelector('[aria-label="选择模型"]')!);
  await click(button("默认（推荐）"));
  await typePrompt("use defaults"); await send();
  expect(client.submitTurn).toHaveBeenLastCalledWith(expect.anything(), "use defaults", null, null, null, [], expect.any(String));
});
it("allows selecting next-turn thinking during an active run without dispatching a turn", async () => {
  const task = snapshot.tasks[0];
  task.activePrimaryRun = { runId: "run_active", turnId: "trn_active", executor: task.preferredExecutor, state: "running", startedAt: new Date().toISOString() };
  await sync();
  expect(element.querySelector('[aria-label="停止"]')).not.toBeNull();
  await click(element.querySelector('[aria-label="选择思考强度"]')!);
  await click(button("低"));
  expect(client.submitTurn).not.toHaveBeenCalled();
  task.activePrimaryRun = null; await sync();
  await typePrompt("lower effort"); await send();
  expect(client.submitTurn).toHaveBeenLastCalledWith(expect.anything(), "lower effort", "old", "low", null, [], expect.any(String));
});

it.each([false, true])("restores a rejected selection without overwriting a newer draft (new draft: %s)", async newer => {
  await click(element.querySelector('[aria-label="选择模型"]')!);
  await click(button("New model"));
  await typePrompt("use new model"); await send();
  if (newer) {
    await click(element.querySelector('[aria-label="选择模型"]')!);
    await click(button("Old model"));
    await click(element.querySelector('[aria-label="选择思考强度"]')!);
    await click(button("低"));
  }
  await act(async () => {
    notify({ type: "event", data: { schemaVersion: 1, sequence: ++snapshot.lastSequence, eventId: `evt_${snapshot.lastSequence}`, stream: { streamType: "task", streamId: snapshot.tasks[0].taskId }, streamVersion: snapshot.lastSequence, eventType: "turn.failed", occurredAt: new Date().toISOString(), recordedAt: new Date().toISOString(), correlationId: "cor_test", causationId: null, source: { kind: "hub" }, payload: { failureKind: "selection_rejected" }, vendorExtensions: null } });
    await new Promise(resolve => setTimeout(resolve, 70));
  });
  expect(element.querySelector('[aria-label="选择模型"]')?.textContent).toContain("Old model");
  await typePrompt("retry"); await send();
  expect(client.submitTurn).toHaveBeenLastCalledWith(expect.anything(), "retry", "old", newer ? "low" : "high", null, [], expect.any(String));
});

it("refreshes a failed model's catalog and preserves the choice until the user replaces it", async () => {
  const available = vi.mocked((client as DemoCoreClient & import("@dogcodes/protocol").CoreClient).availableModels!);
  const before = available.mock.calls.length;
  available.mockResolvedValue({defaultModel:"old", models:[{id:"new",label:"New model",thinkingOptions:[],defaultThinking:null}]});
  await act(async()=>{
    notify({type:"event",data:{schemaVersion:1,sequence:++snapshot.lastSequence,eventId:`evt_${snapshot.lastSequence}`,stream:{streamType:"task",streamId:snapshot.tasks[0].taskId},streamVersion:snapshot.lastSequence,eventType:"turn.failed",occurredAt:new Date().toISOString(),recordedAt:new Date().toISOString(),correlationId:"cor_test",causationId:null,source:{kind:"hub"},payload:{failureKind:"model_unavailable"},vendorExtensions:null}});
    await new Promise(resolve=>setTimeout(resolve,70));
  });
  expect(available.mock.calls.length).toBeGreaterThan(before);
  await click(element.querySelector('[aria-label="选择模型"]')!);
  expect(element.textContent).toContain("已选模型不在当前列表中");
  expect(button("默认模型不可用").disabled).toBe(true);
  await click(button("New model"));
  await typePrompt("retry with new model"); await send();
  expect(client.submitTurn).toHaveBeenLastCalledWith(expect.anything(),"retry with new model","new",null,null,[],expect.any(String));
});

it.each([null, "ins_old_cursor"])("loads the restored Agent's models when its installation is %s", async (installationId) => {
  const executor = { providerId: "cursor", installationId: "ins_current_cursor" } as const;
  snapshot.tasks[0].preferredExecutor = { providerId: "cursor", installationId };
  snapshot.installations = [{ ...snapshot.installations[0], ...executor }];
  snapshot.sessionMaps = [];
  const availableModels = vi.fn().mockResolvedValue({ defaultModel: "auto", models: [
    { id: "auto", label: "Cursor Auto", thinkingOptions: [], defaultThinking: null },
  ] });
  const permissionCatalog = vi.fn().mockImplementation(async (target) => {
    if (target.installationId !== executor.installationId) throw Error("driver unavailable");
    return { defaultModeId: "ask", modes: [{ id: "ask", label: "Ask", description: "", dangerous: false }] };
  });
  Object.assign(client, { availableModels, permissionCatalog, modelReadyExecutors: vi.fn().mockResolvedValue([executor]) });
  // A remount models a reload: the saved task still refers to the old installation,
  // while discovery and runtime registration expose only the current executable.
  await act(async () => root.render(<App key="reload" client={client} />));
  expect(element.querySelector('[aria-label="选择模型"]')?.textContent).toContain("Cursor Auto");
  expect(permissionCatalog).toHaveBeenCalledWith(executor, snapshot.tasks[0].taskId);
  expect(element.querySelector('[aria-label="权限策略"]')?.textContent).not.toContain("不可用");
  await click(element.querySelector('[aria-label="选择模型"]')!);
  expect(element.querySelector('.model-menu')?.textContent).not.toContain("正在获取");
  expect(availableModels).toHaveBeenCalledTimes(1);
  expect(availableModels).toHaveBeenCalledWith(executor, null, snapshot.tasks[0].taskId);
});

it.each([false, true])("recovers a replaced installation after delayed registration (draft: %s)", async (draft) => {
  let registered = [snapshot.tasks[0].preferredExecutor];
  const listeners = new Set<() => void>();
  const availableModels = vi.fn().mockImplementation(async executor => ({ defaultModel: "auto", models: [
    { id: "auto", label: executor.installationId === "ins_replaced" ? "Recovered model" : "Original model", thinkingOptions: [], defaultThinking: null },
  ] }));
  Object.assign(client, {
    availableModels,
    modelReadyExecutors: vi.fn(async () => registered),
    onQuotaProviderReady: async (callback: () => void) => { listeners.add(callback); return () => { listeners.delete(callback); }; },
  });
  snapshot.sessionMaps = [];
  await act(async () => root.render(<App key="reload" client={client} />));
  if (draft) await click(button("新建对话"));
  await typePrompt("keep my draft");
  expect(element.querySelector('[aria-label="选择模型"]')?.textContent).toContain("Original model");
  const executor = { providerId: "codex", installationId: "ins_replaced" } as const;
  snapshot.installations = [{ ...snapshot.installations[0], ...executor }];
  await sync();
  await click(element.querySelector('[aria-label="选择模型"]')!);
  expect(element.querySelector('.model-menu')?.textContent).toContain("正在获取");
  registered = [executor];
  await act(async () => { listeners.forEach(callback => callback()); });
  expect(element.querySelector('[aria-label="选择模型"]')?.textContent).toContain("Recovered model");
  expect(element.querySelector('.model-menu')?.textContent).not.toContain("正在获取");
  expect(element.querySelector('textarea')?.value).toBe("keep my draft");
  expect(availableModels).toHaveBeenLastCalledWith(executor, null, draft ? undefined : snapshot.tasks[0].taskId);
  // A new draft queries native defaults without sending its synthetic view-model ID.
  expect(availableModels).toHaveBeenCalledTimes(draft ? 3 : 2);
});

it.each(["antigravity", "claude-code", "codebuddy", "codex", "cursor", "deepseek-harness", "grok", "kimi", "kiro", "opencode", "pi", "zcode"])(
  "loads %s models and thinking for an unsent conversation without a synthetic task ID", async providerId => {
    const executor = { providerId, installationId: `ins_${providerId}` } as const;
    snapshot.tasks[0].preferredExecutor = executor;
    snapshot.installations = [{ ...snapshot.installations[0], ...executor }];
    snapshot.sessionMaps = [];
    const availableModels = vi.fn(async (_executor, _model, taskId) => {
      // Match the real backend boundary: temporary UI IDs are not native task IDs.
      if (taskId === "tsk_draft") throw Error("invalid ULID in id tsk_draft");
      return { defaultModel: "native", models: [{ id: "native", label: "Native model",
        thinkingOptions: [{ id: "high", label: "High" }], defaultThinking: "high" }] };
    });
    Object.assign(client, { availableModels, modelReadyExecutors: vi.fn(async () => [executor]) });
    await act(async () => root.render(<App key={providerId} client={client} />));
    await click(button("新建对话"));
    expect(availableModels).toHaveBeenLastCalledWith(executor, null, undefined);
    await click(element.querySelector('[aria-label="选择模型"]')!);
    expect(element.querySelector('.model-menu')?.textContent).toContain("Native model");
    expect(element.querySelector('.model-menu')?.textContent).not.toContain("暂时无法获取模型");
    expect(element.querySelector('.model-menu')?.textContent).not.toContain("正在获取");
    await click(element.querySelector('[aria-label="选择思考强度"]')!);
    expect(element.querySelector('.model-menu')?.textContent).toContain("高");
    expect(client.submitTurn).not.toHaveBeenCalled();
  },
);

it("keeps the running executor until the turn completes before reconciling installations", async () => {
  const task = snapshot.tasks[0];
  task.activePrimaryRun = { runId: "run_active", turnId: "trn_active", executor: task.preferredExecutor, state: "running", startedAt: new Date().toISOString() };
  snapshot.installations = [{ ...snapshot.installations[0], installationId: "ins_replaced" }];
  await sync();
  expect(element.querySelector('[aria-label="选择模型"]')?.textContent).toContain("Old model");
  expect(element.querySelector('[aria-label="停止"]')).not.toBeNull();
  snapshot.sessionMaps = [];
  task.activePrimaryRun = null;
  await sync();
  await click(element.querySelector('[aria-label="选择 Coding Agent"]')!);
  expect(element.querySelector('.agent-picker-option[aria-pressed="true"]')?.getAttribute("aria-label")).toBe("Codex");
});

it("queries the selected model's thinking options and refreshes without resetting saved choices", async () => {
  const availableModels = vi.fn(async (_executor, model) => ({ defaultModel: "old", models: [
    { id: "old", label: "Old model", thinkingOptions: [{id:"high",label:"High"}], defaultThinking:"high" },
    { id: "new", label: "New model", thinkingOptions: model === "new" ? [{id:"low",label:"Low"}] : [], defaultThinking: model === "new" ? "low" : null },
  ] }));
  Object.assign(client,{availableModels});
  await act(async()=>root.render(<App key="efforts" client={client}/>));
  expect(availableModels).toHaveBeenLastCalledWith(snapshot.tasks[0].preferredExecutor,"old",snapshot.tasks[0].taskId);
  await click(element.querySelector('[aria-label="选择模型"]')!);await click(button("New model"));
  expect(availableModels).toHaveBeenLastCalledWith(snapshot.tasks[0].preferredExecutor,"new",snapshot.tasks[0].taskId);
  await click(element.querySelector('[aria-label="选择思考强度"]')!);
  await click(element.querySelector('[aria-label="刷新思考强度列表"]')!);
  expect(element.querySelector('[aria-label="选择模型"]')?.textContent).toContain("New model");
  expect(element.querySelector('.model-options')?.textContent).toContain("低");
  expect(client.submitTurn).not.toHaveBeenCalled();
});
