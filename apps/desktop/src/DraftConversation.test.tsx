// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { setAgentEnabled } from "./agentPreferences";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";
import { createHash, webcrypto } from "node:crypto";
import type { ArtifactChunk, ArtifactMetadata, TimelineItem } from "@dogcodes/protocol";
import { loadTurnArtifact } from "./turnArtifactCache";

let root: Root, element: HTMLDivElement, client: DemoCoreClient;
const button = (label: string) => [...element.querySelectorAll("button")].find(b => b.textContent?.includes(label))!;
const click = async (target: Element) => act(async () => { (target as HTMLElement).click(); });
const typePrompt = async (text: string) => act(async () => {
  const input = element.querySelector("textarea")!;
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(input, text);
  input.dispatchEvent(new Event("input", { bubbles: true }));
});
const send = async () => act(async () => { element.querySelector("form.composer")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
beforeEach(async () => {
  localStorage.clear();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
  client = new DemoCoreClient();
  vi.spyOn(client, "createTask");
  vi.spyOn(client, "submitTurn");
  vi.spyOn(client, "timeline");
  element = document.createElement("div"); document.body.append(element); root = createRoot(element);
  await act(async () => root.render(<App client={client} />));
});
afterEach(async () => { await act(async () => root.unmount()); element.remove(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

const pressShortcut = async (key: string, modifiers: KeyboardEventInit) => {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...modifiers });
  await act(async () => { (element.querySelector("textarea") ?? window).dispatchEvent(event); });
  return event;
};

it.each(["MacIntel", "Win32"])("opens quota and a new draft with the %s shortcuts", async platform => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue(platform);
  await act(async () => root.render(<App client={client} />));
  const mac = platform === "MacIntel";
  const modifiers = mac ? { metaKey: true } : { ctrlKey: true };
  expect(button("新建对话").querySelector("kbd")?.textContent).toBe(mac ? "⌘ N" : "Ctrl N");
  expect(button("Agent 管理").querySelector("kbd")?.textContent).toBe(mac ? "⌘ U" : "Ctrl U");
  expect(button("Agent 管理").querySelector("small")).toBeNull();
  expect((await pressShortcut("u", modifiers)).defaultPrevented).toBe(true);
  expect(element.textContent).toContain("拖拽调整顺序");
  expect((await pressShortcut("n", modifiers)).defaultPrevented).toBe(true);
  expect(element.querySelector(".welcome-stage")).not.toBeNull();
  expect(element.querySelector("textarea")?.value).toBe("");
  expect(client.createTask).not.toHaveBeenCalled();
});

it("ignores other modifiers, composition, repeats, dialogs and disabled actions", async () => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue("Win32");
  await click(button("新建对话"));
  await typePrompt("保留正在输入的草稿");
  for (const modifiers of [
    {}, { metaKey: true }, { ctrlKey: true, shiftKey: true },
    { ctrlKey: true, altKey: true }, { ctrlKey: true, isComposing: true },
    { ctrlKey: true, repeat: true },
  ]) await pressShortcut("n", modifiers);
  expect(element.querySelector("textarea")?.value).toBe("保留正在输入的草稿");
  const dialog = document.createElement("dialog"); dialog.setAttribute("open", ""); document.body.append(dialog);
  try {
    await pressShortcut("n", { ctrlKey: true });
    await pressShortcut("u", { ctrlKey: true });
    expect(element.querySelector("textarea")?.value).toBe("保留正在输入的草稿");
    expect(button("Agent 管理").classList.contains("active")).toBe(false);
  } finally { dialog.remove(); }
  let reject!: (error: Error) => void;
  vi.mocked(client.createTask).mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
  await send();
  expect(button("正在创建").disabled).toBe(true);
  await pressShortcut("n", { ctrlKey: true });
  await act(async () => reject(new Error("测试创建失败")));
  expect(element.querySelector("textarea")?.value).toBe("");
  expect(element.querySelector('[aria-label="待核实的发送请求"]')?.textContent).toContain("保留正在输入的草稿");
});

it("keeps repeated new conversations and project choices local until the first message", async () => {
  const original = (await client.bootstrap()).tasks.length;
  await click(button("新建对话")); await click(button("新建对话"));
  expect(client.createTask).not.toHaveBeenCalled();
  expect(client.timeline).not.toHaveBeenCalledWith("tsk_draft");
  await click(element.querySelector('[aria-label="选择项目目录"]')!);
  const project = (await client.bootstrap()).projects.find(p => p.displayName === "dogcodes")!;
  await click(element.querySelector(".project-picker-list button")!);
  expect(client.createTask).not.toHaveBeenCalled();
  expect((await client.bootstrap()).tasks).toHaveLength(original);
  await typePrompt("开始项目任务"); await send();
  expect(client.createTask).toHaveBeenCalledExactlyOnceWith(project.projectId, "新对话", expect.any(Object), expect.objectContaining({ operationId: expect.any(String), taskId: expect.any(String) }));
  expect(client.submitTurn).toHaveBeenCalledWith(expect.objectContaining({ projectId: project.projectId }), "开始项目任务", null, null, null, [], expect.any(String));
  expect((await client.bootstrap()).tasks).toHaveLength(original + 1);
});

it("creates an unassigned conversation only on send and explicitly recovers its original send", async () => {
  const scratch = (await client.bootstrap()).projects.find(p => p.displayName === "快速对话")!;
  vi.mocked(client.submitTurn).mockRejectedValueOnce(new Error("发送失败测试"));
  await click(button("新建对话"));
  await typePrompt("无项目消息"); await send();
  expect(client.createTask).toHaveBeenCalledExactlyOnceWith(scratch.projectId, "新对话", expect.any(Object), expect.objectContaining({ operationId: expect.any(String), taskId: expect.any(String) }));
  expect(element.querySelector("textarea")?.value).toBe("");
  expect(element.querySelector('[aria-label="待核实的发送请求"]')?.textContent).toContain("无项目消息");
  await click(button("核实发送结果"));
  await click(button("继续原发送"));
  expect(client.createTask).toHaveBeenCalledTimes(1);
  expect(client.submitTurn).toHaveBeenCalledTimes(2);
});

it("shows a blank conversation area instead of the welcome page while history loads", async () => {
  const snapshot = await client.bootstrap();
  const task = snapshot.tasks[0];
  const items = await client.timeline(task.taskId);
  let finish!: (result: typeof items) => void;
  vi.mocked(client.timeline).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  await act(async () => root.unmount());
  root = createRoot(element);
  await act(async () => root.render(<App client={client} />));
  expect(element.querySelector(".conversation-loading")).not.toBeNull();
  expect(element.querySelector(".welcome-stage")).toBeNull();
  expect(element.querySelector(".composer")).toBeNull();
  await act(async () => finish(items));
  expect(element.querySelector(".conversation-loading")).toBeNull();
  expect(element.querySelector(".composer")).not.toBeNull();
});

it("accepts pasted files and workspace drops, removes cards, and preserves attachments on send failure", async () => {
  await click(button("新建对话"));
  const paste = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(paste, "clipboardData", { value: { files: [new File(["document"], "report.docx")], getData: () => "" } });
  await act(async () => { element.querySelector("textarea")!.dispatchEvent(paste); });
  expect(paste.defaultPrevented).toBe(true);
  expect(element.querySelector(".attachment-details")?.textContent).toBe("report.docxDOCX");
  expect(element.querySelector<HTMLButtonElement>('[aria-label="发送"]')!.disabled).toBe(false);
  await send();
  expect(element.textContent).toContain("请在桌面应用中发送附件");
  expect(element.querySelectorAll(".attachment-card")).toHaveLength(1);
  expect(client.submitTurn).not.toHaveBeenCalled();
  await click(element.querySelector('[aria-label="移除附件 report.docx"]')!);
  expect(element.querySelectorAll(".attachment-card")).toHaveLength(0);
  const drop = new Event("drop", { bubbles: true, cancelable: true });
  Object.defineProperty(drop, "dataTransfer", { value: { files: [new File(["hello"], "notes.txt")] } });
  await act(async () => { element.querySelector(".welcome-stage")!.dispatchEvent(drop); });
  expect(drop.defaultPrevented).toBe(true);
  expect(element.querySelector(".attachment-details")?.textContent).toBe("notes.txtTXT");
  const textPaste = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(textPaste, "clipboardData", { value: { files: [], getData: () => "ordinary text" } });
  element.querySelector("textarea")!.dispatchEvent(textPaste);
  expect(textPaste.defaultPrevented).toBe(false);
});

it("restores an already loaded conversation without requesting its timeline again", async () => {
  const timeline = element.querySelector(".timeline");
  vi.mocked(client.timeline).mockClear();
  await click(button("新建对话"));
  await click(element.querySelector(".history-row .task")!);
  expect(client.timeline).not.toHaveBeenCalled();
  expect(element.querySelector(".conversation-loading")).toBeNull();
  expect(element.querySelector(".timeline")).toBe(timeline);
});

it.each([false, true])("shows selected history before a slow file read settles (failure=%s)", async failure => {
  await act(async () => root.unmount());
  const snapshot = await client.bootstrap();
  const original = snapshot.tasks[0];
  const task = { ...original, taskId: "tsk_slow_history" as const, title: "慢速历史会话" };
  vi.spyOn(client, "bootstrap").mockResolvedValue({ ...snapshot, tasks: [original, task] });
  const bytes = new TextEncoder().encode(JSON.stringify({ files: [{ path: "history.ts", status: "M" }] }));
  const metadata: ArtifactMetadata = {
    artifactId: "art_slow_history", byteLength: bytes.length,
    payloadSha256: createHash("sha256").update(bytes).digest("hex"), mediaType: "application/json",
    createdAt: "2026-09-26T00:00:00Z", redactionRulesetVersion: 1, redactionCount: 0,
  };
  const message: TimelineItem = { taskId: task.taskId, itemId: "history-answer", turnId: "trn_history",
    kind: "assistant_message", text: "历史正文立即可读", providerId: "codex", attachments: [], sequence: 1, occurredAt: metadata.createdAt };
  const originalItems = await client.timeline(original.taskId);
  vi.mocked(client.timeline).mockImplementation(async id => id === task.taskId
    ? [message, { ...message, itemId: "history-files", kind: "turn_files", text: JSON.stringify(metadata), sequence: 2 }]
    : originalItems);
  vi.stubGlobal("crypto", webcrypto);
  let finish!: (chunk: ArtifactChunk) => void, fail!: (error: Error) => void;
  const read = vi.spyOn(client, "readArtifact").mockImplementation(() => new Promise((resolve, reject) => { finish = resolve; fail = reject; }));
  root = createRoot(element);
  await act(async () => root.render(<App client={client} />));
  await click(button(task.title));
  expect(read).toHaveBeenCalledTimes(1);
  expect(element.querySelector(".conversation-loading")).toBeNull();
  const visibleTimeline = [...element.querySelectorAll(".timeline")].find(node => !node.closest("[hidden]"));
  expect(visibleTimeline?.textContent).toContain("历史正文立即可读");
  expect(element.querySelector(".composer")).not.toBeNull();
  await act(async () => {
    const pending = loadTurnArtifact(client, task.taskId, metadata).catch(() => {});
    if (failure) fail(new Error("slow read failed"));
    else finish({ metadata, bytes: [...bytes], offset: 0, nextOffset: null });
    await pending;
  });
  expect(element.textContent).toContain(failure ? "本轮变更加载失败" : "1 个文件已更改");
  expect(element.textContent).toContain("历史正文立即可读");
  expect(element.querySelector(".conversation-loading")).toBeNull();
  await click(button(original.title));
  expect(element.querySelector('.history-row.active .task')?.textContent).toContain(original.title);
  expect(element.querySelector(".conversation-loading")).toBeNull();
});

it("shows an agent loading indicator immediately while submit is pending and clears it on failure", async () => {
  let reject!: (error: Error) => void;
  vi.mocked(client.submitTurn).mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
  await typePrompt("等待首个回复");
  await send();
  expect(element.querySelector('.timeline .agent-pending [role="img"]')).not.toBeNull();
  expect(element.querySelector('.agent-pending')?.textContent).toContain("正在响应");
  await act(async () => reject(new Error("测试发送失败")));
  expect(element.querySelector('.agent-pending')).toBeNull();
  expect(element.querySelector('textarea')?.value).toBe("");
  expect(element.querySelector('[aria-label="待核实的发送请求"]')?.textContent).toContain("等待首个回复");
});

it("shows the selected agent even before a new conversation has been created", async () => {
  let reject!: (error: Error) => void;
  vi.mocked(client.createTask).mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
  await click(button("新建对话"));
  await typePrompt("首次消息");
  await send();
  expect(element.querySelector('.workspace.has-history .timeline .agent-pending [role="img"]')).not.toBeNull();
  expect(element.querySelector('.composer-dock textarea')).not.toBeNull();
  expect(element.querySelector('.welcome-stage')).toBeNull();
  expect(element.querySelector('.welcome-stage h2')).toBeNull();
  await act(async () => reject(new Error("测试创建失败")));
  expect(element.querySelector('.agent-pending')).toBeNull();
});

it("refreshes all Agent quotas from the quota page and shows progress until completion", async () => {
  await click(button("Agent 管理"));
  const views = await client.quotaSnapshots();
  let finish!: (value: typeof views) => void;
  const pending = new Promise<typeof views>(resolve => { finish = resolve; });
  const read = vi.spyOn(client, "quotaSnapshots").mockReturnValue(pending);
  await click(button("刷新额度"));
  expect(button("正在刷新").disabled).toBe(true);
  expect(element.querySelector('[role="status"]')?.textContent).toContain("正在刷新");
  const count = (await client.bootstrap()).installations.filter(i => i.providerId !== "fake").length;
  expect(read).toHaveBeenCalledTimes(count);
  expect(read.mock.calls.every(call => (call as unknown[])[2] === true)).toBe(true);
  await click(button("正在刷新"));
  expect(read).toHaveBeenCalledTimes(count);
  await act(async () => finish(views));
  expect(button("刷新额度").disabled).toBe(false);
  expect(element.querySelector('.quota-refresh-status')?.textContent).toMatch(/已刷新|已完成刷新/);
});

const visible = (selector: string) => [...element.querySelectorAll(selector)].filter(node => !node.closest("[hidden]"));
const expectSubmittedLayout = (prompt: string) => {
  expect(element.querySelector(".workspace.has-history")).not.toBeNull();
  expect(element.querySelector(".welcome-stage")).toBeNull();
  expect(element.querySelector(".conversation-loading")).toBeNull();
  expect(element.querySelector(".composer-dock textarea")).not.toBeNull();
  expect(visible(".user-message").filter(node => node.textContent === prompt)).toHaveLength(1);
};

it("keeps the sent layout through task creation, empty history, acknowledgement and delayed canonical history", async () => {
  vi.useFakeTimers();
  await click(button("新建对话"));
  const createTask = client.createTask.bind(client);
  let finishCreation!: () => void, finishSubmit!: () => void;
  vi.mocked(client.createTask).mockImplementationOnce(async (...args) => {
    await new Promise<void>(resolve => { finishCreation = resolve; });
    return createTask(...args);
  });
  let finishHistory!: (items: TimelineItem[]) => void;
  vi.mocked(client.timeline).mockImplementation(() => new Promise(resolve => { finishHistory = resolve; }));
  vi.mocked(client.submitTurn).mockImplementationOnce(async (...args) => {
    await new Promise<void>(resolve => { finishSubmit = resolve; });
    await DemoCoreClient.prototype.submitTurn.apply(client, args);
  });
  await typePrompt("立即进入对话布局"); await send();
  expectSubmittedLayout("立即进入对话布局");
  expect(element.querySelector("textarea")?.value).toBe("");
  expect(visible(".agent-pending")).toHaveLength(1);
  await act(async () => finishCreation());
  expectSubmittedLayout("立即进入对话布局");
  // The service creates the conversation and sends in one call; the new
  // conversation is selected once that call returns.
  await act(async () => { finishSubmit(); await vi.advanceTimersByTimeAsync(800); });
  expectSubmittedLayout("立即进入对话布局");
  await act(async () => finishHistory([]));
  expectSubmittedLayout("立即进入对话布局");
  expect(visible(".agent-pending")).toHaveLength(1);
  const task = vi.mocked(client.submitTurn).mock.calls[0][0];
  const canonical = (await DemoCoreClient.prototype.timeline.call(client, task.taskId)).find(item => item.kind === "user_message")!;
  const message: TimelineItem = { taskId: task.taskId, itemId: "canonical-prompt", turnId: canonical.turnId,
    kind: "user_message", text: "立即进入对话布局", providerId: null, attachments: [], sequence: canonical.sequence, occurredAt: new Date().toISOString() };
  // The create event schedules a projection refresh; deliver the canonical
  // prompt together with the first streamed response on that next read.
  await act(async () => { await vi.advanceTimersByTimeAsync(60); });
  await act(async () => finishHistory([message, { ...message, itemId: "first-reply", kind: "assistant_message", text: "正在检查项目", sequence: 101 }]));
  expectSubmittedLayout("立即进入对话布局");
  expect(visible(".agent-pending")).toHaveLength(0);
  expect(visible(".assistant-message")[0]?.textContent).toContain("正在检查项目");
});

it("removes the optimistic prompt and retains the original send after task creation becomes uncertain", async () => {
  await click(button("新建对话"));
  let reject!: (error: Error) => void;
  vi.mocked(client.createTask).mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
  await typePrompt("失败时保留消息"); await send();
  expectSubmittedLayout("失败时保留消息");
  await act(async () => reject(new Error("创建失败")));
  expect(visible(".user-message")).toHaveLength(0);
  expect(visible(".agent-pending")).toHaveLength(0);
  expect(element.querySelector(".welcome-stage textarea")).not.toBeNull();
  expect(element.querySelector("textarea")?.value).toBe("");
  expect(element.querySelector('[aria-label="待核实的发送请求"]')?.textContent).toContain("失败时保留消息");
  expect(element.querySelector('[role="alert"]')?.textContent).toContain("创建失败");
});
it("starts an empty draft in the project selected by the sidebar plus button", async () => {
  await typePrompt("旧会话的输入");
  const project = (await client.bootstrap()).projects.find(project => project.displayName !== "快速对话")!;
  await click(element.querySelector(`[aria-label="在 ${project.displayName} 中新建会话"]`)!);
  expect(element.querySelector("textarea")?.value).toBe("");
  expect(client.createTask).not.toHaveBeenCalled();
  await typePrompt("项目的新会话");
  await send();
  expect(client.createTask).toHaveBeenCalledExactlyOnceWith(project.projectId, "新对话", expect.any(Object), expect.objectContaining({ operationId: expect.any(String), taskId: expect.any(String) }));
});

it("blocks new submissions when every installed Agent is disabled", async () => {
  const snapshot = await client.bootstrap();
  const providers = snapshot.installations.map(item => item.providerId);
  try {
    await act(async () => providers.forEach(id => setAgentEnabled(id, false)));
    await click(button("新建对话"));
    await typePrompt("不应发送给已禁用的 Agent");
    expect(element.querySelector<HTMLButtonElement>('button[aria-label="发送"]')!.disabled).toBe(true);
    await send();
    expect(client.createTask).not.toHaveBeenCalled();
    expect(client.submitTurn).not.toHaveBeenCalled();
    expect(element.querySelectorAll('.agent-picker-option')).toHaveLength(0);
  } finally { await act(async () => providers.forEach(id => setAgentEnabled(id, true))); }
});

it("does not overwrite a newer draft when an older creation fails", async () => {
  await click(button("新建对话"));
  let reject!: (cause: Error) => void;
  vi.mocked(client.createTask).mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
  await typePrompt("original request"); await send();
  await typePrompt("newer draft");
  await act(async () => reject(new Error("lost response")));
  expect(element.querySelector("textarea")?.value).toBe("newer draft");
  expect(element.querySelector('[aria-label="待核实的发送请求"]')?.textContent).toContain("original request");
});

it("keeps unresolved sends visible after reopening without creating or sending again", async () => {
  vi.mocked(client.submitTurn).mockRejectedValueOnce(new Error("offline"));
  await typePrompt("recover after reopen"); await send();
  const calls = vi.mocked(client.submitTurn).mock.calls.length;
  await act(async () => root.unmount()); root = createRoot(element);
  await act(async () => root.render(<App client={client}/>));
  expect(element.querySelector('[aria-label="待核实的发送请求"]')?.textContent).toContain("recover after reopen");
  expect(client.submitTurn).toHaveBeenCalledTimes(calls);
  await typePrompt("another message"); await send();
  expect(client.submitTurn).toHaveBeenCalledTimes(calls);
  expect(element.querySelector<HTMLButtonElement>('[aria-label="发送"]')?.disabled).toBe(true);
});

it("does not mistake another command's identical message for the pending submission", async () => {
  vi.useFakeTimers();
  const task = (await client.bootstrap()).tasks[0];
  let reject!: (cause: Error) => void;
  vi.mocked(client.submitTurn).mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
  await typePrompt("identical message"); await send();
  await act(async () => {
    const external = DemoCoreClient.prototype.submitTurn.call(client, task, "identical message", null, null, null, [], "cmd_other_window");
    await vi.advanceTimersByTimeAsync(850); await external;
  });
  expect(visible(".user-message").filter(node => node.textContent === "identical message")).toHaveLength(2);
  await act(async () => reject(new Error("version changed")));
  expect(visible(".user-message").filter(node => node.textContent === "identical message")).toHaveLength(1);
  expect(element.querySelector('[aria-label="待核实的发送请求"]')?.textContent).toContain("identical message");
});

it("does not restore a failed old send into a different selected conversation", async () => {
  await click(button("新建对话"));
  let reject!: (cause: Error) => void;
  vi.mocked(client.createTask).mockImplementationOnce(() => new Promise((_, fail) => { reject = fail; }));
  await typePrompt("old conversation request"); await send();
  await click(element.querySelector(".history-row .task")!);
  await typePrompt("draft for selected conversation");
  await act(async () => reject(new Error("old creation failed")));
  expect(element.querySelector("textarea")?.value).toBe("draft for selected conversation");
  expect(element.querySelector("form.composer [role=alert]")).toBeNull();
  expect(element.querySelector('[aria-label="待核实的发送请求"]')?.textContent).toContain("old conversation request");
});

it("keeps a send with an unknown outcome out of the composer and blocks a fresh send until it is verified", async () => {
  const start = vi.spyOn(client, "submissionStart").mockRejectedValueOnce(new Error("reply lost"));
  const lookup = vi.spyOn(client, "submissionIntentGet").mockRejectedValueOnce(new Error("service restarting"));
  await typePrompt("结果未知的消息"); await send();
  const original = start.mock.calls[0][0].operationId;
  expect(element.querySelector("textarea")?.value).toBe("");
  expect(element.querySelector('[aria-label="结果未知的发送请求"]')?.textContent).toContain("结果未知的消息");
  await typePrompt("另一条消息"); await send();
  expect(start).toHaveBeenCalledTimes(1);
  // Verification finds no record: resending reuses the original identity.
  lookup.mockResolvedValueOnce(null);
  await click(button("核实发送结果"));
  await act(async () => { button("重新发送原内容").click(); await new Promise(resolve => setTimeout(resolve, 900)); });
  expect(start).toHaveBeenCalledTimes(2);
  expect(start.mock.calls[1][0].operationId).toBe(original);
  expect(element.querySelector('[aria-label="结果未知的发送请求"]')).toBeNull();
});

it("returns the text to the draft only after verifying the service never received it", async () => {
  vi.spyOn(client, "submissionStart").mockRejectedValueOnce(new Error("reply lost"));
  const lookup = vi.spyOn(client, "submissionIntentGet").mockRejectedValueOnce(new Error("offline"));
  await typePrompt("要放回的消息"); await send();
  expect(element.querySelector("textarea")?.value).toBe("");
  lookup.mockResolvedValueOnce(null);
  await click(button("核实发送结果"));
  await click(button("放弃并放回输入框"));
  expect(element.querySelector("textarea")?.value).toBe("要放回的消息");
  expect(element.querySelector('[aria-label="结果未知的发送请求"]')).toBeNull();
});
