import { invoke } from "@tauri-apps/api/core";
export type GitAction = "commit" | "push" | "pull" | "sync";
export interface GitWorkflow { nextOperationId: string; parentOperationId?: string | null }
export interface GitRequest { operationId: string; taskId: string; rootId: string; action: GitAction; message: string; stageAll: boolean; snapshotId: string | null; workflow?: GitWorkflow | null }
export interface GitReceipt { closed?: boolean; request: GitRequest; outcome: { status: "running" | "unknown" } | { status: "succeeded"; completed: boolean; targetError?: string | null } | { status: "failed"; message: string } }
export interface PendingGit { request: GitRequest; intent: GitAction | "commitPush"; nextOperationId?: string; retryOperationId?: string; committed?: boolean; receipt?: GitReceipt | null; sending?: boolean }
const prefix = "dogcodes:pending-git:";
const listeners = new Set<() => void>();
const sending = new Set<string>();
const key = (id: string) => `${prefix}${id}`;
function notify() { listeners.forEach(listener => listener()); }
function store(value: PendingGit) { localStorage.setItem(key(value.request.operationId), JSON.stringify(value)); notify(); }
function parse(value: string): PendingGit {
  const item = JSON.parse(value) as PendingGit;
  if (!item.request?.operationId || !item.request.taskId || !item.request.rootId || !["commit", "push", "pull", "sync"].includes(item.request.action)) throw new Error("待核实的 Git 操作记录损坏，请保留记录并检查本机存储");
  return item;
}
function entries(taskId: string) {
  const result: PendingGit[] = [];
  for (let i = 0; i < localStorage.length; i++) {
    const name = localStorage.key(i);
    if (!name?.startsWith(prefix)) continue;
    const raw = localStorage.getItem(name); if (!raw) continue;
    const item = parse(raw); if (item.request.taskId === taskId) result.push(item);
  }
  return result.sort((a, b) => a.request.operationId.localeCompare(b.request.operationId));
}
function remove(value: PendingGit) { localStorage.removeItem(key(value.request.operationId)); notify(); }
function sameRequest(a: GitRequest, b: GitRequest) {
  return a.operationId === b.operationId && a.taskId === b.taskId && a.rootId === b.rootId && a.action === b.action && a.message === b.message
    && (a.workflow?.nextOperationId ?? null) === (b.workflow?.nextOperationId ?? null) && (a.workflow?.parentOperationId ?? null) === (b.workflow?.parentOperationId ?? null)
    && Boolean(a.stageAll) === Boolean(b.stageAll) && (a.snapshotId ?? null) === (b.snapshotId ?? null);
}
async function receipt(value: PendingGit): Promise<GitReceipt | null> {
  const result = await invoke<GitReceipt | null>("core_review_git_operation", { taskId: value.request.taskId, operationId: value.request.operationId });
  if (result && !sameRequest(result.request, value.request)) throw new Error("Git 回执与原请求不匹配，未重发操作");
  return result;
}
export class GitResultUnknown extends Error {
  constructor() { super("Git 操作结果尚未核实，请先核实结果，避免重复操作"); }
}
export class GitPushFailed extends Error {}
export class GitWorkflowClosed extends Error { constructor() { super("此次 Git 操作已在后台结束，不再接续推送。"); } }
function settle(value: PendingGit, result: GitReceipt): boolean {
  if (result.closed) { remove(value); throw new GitWorkflowClosed(); }
  if (result.outcome.status === "succeeded") {
    if (value.intent === "commitPush" && value.request.action === "commit" && result.outcome.completed) { if (localStorage.getItem(key(value.request.operationId))) store({ ...value, receipt: result }); }
    else remove(value);
    return result.outcome.completed;
  }
  if (result.outcome.status === "failed") {
    if (value.committed && value.request.action === "push") {
      if (localStorage.getItem(key(value.request.operationId))) store({ ...value, receipt: result });
      throw new GitPushFailed(result.outcome.message);
    }
    remove(value); throw new Error(result.outcome.message);
  }
  throw new GitResultUnknown();
}
async function sendRequest(value: PendingGit): Promise<boolean> {
  let completed: boolean;
  try {
    completed = await invoke<boolean>("core_review_git_action", value.request as unknown as Record<string, unknown>);
  } catch {
    let result: GitReceipt | null;
    try { result = await receipt(value); } catch { throw new GitResultUnknown(); }
    if (result) return settle(value, result);
    throw new GitResultUnknown();
  }
  // A successful response is sent only after the service persists the receipt.
  return settle(value, { request: value.request, outcome: { status: "succeeded", completed } });
}
async function send(value: PendingGit): Promise<boolean> {
  sending.add(value.request.operationId); notify();
  try { return await sendRequest(value); }
  finally { sending.delete(value.request.operationId); notify(); }
}
function fromReceipt(result: GitReceipt): PendingGit {
  const { request } = result, flow = request.workflow;
  return { request, receipt: result, intent: flow && request.action === "commit" ? "commitPush" : request.action,
    ...(flow && request.action === "commit" ? { nextOperationId: flow.nextOperationId } : {}),
    ...(flow && request.action === "push" ? { committed: true, retryOperationId: flow.nextOperationId } : {}) };
}
function validateChild(parent: GitRequest, result: GitReceipt) {
  const child = result.request;
  if (child.operationId !== parent.workflow?.nextOperationId || child.taskId !== parent.taskId || child.rootId !== parent.rootId
    || child.action !== "push" || child.message !== "" || child.stageAll || child.snapshotId
    || child.workflow?.parentOperationId !== parent.operationId || !child.workflow.nextOperationId) {
    throw new Error("Git 接续回执与原请求不匹配，未重发操作");
  }
}
function adoptChild(parent: PendingGit, result: GitReceipt) {
  validateChild(parent.request, result);
  const child = fromReceipt(result); store(child); remove(parent); return child;
}
async function advanceWorkflow(parent: PendingGit): Promise<GitReceipt> {
  if (parent.receipt?.closed) { remove(parent); throw new GitWorkflowClosed(); }
  if (!parent.request.workflow) throw new Error("原操作没有后台组合意图，请核实仓库");
  store(parent); // A local breadcrumb is useful even if the advance reply is lost.
  let result: GitReceipt;
  try {
    result = await invoke<GitReceipt>("core_review_git_advance", { taskId: parent.request.taskId, operationId: parent.request.operationId });
  } catch {
    let known: GitReceipt | null, current: GitReceipt | null;
    try {
      current = await receipt(parent);
      if (current?.closed) { remove(parent); throw new GitWorkflowClosed(); }
      known = await invoke<GitReceipt | null>("core_review_git_operation", { taskId: parent.request.taskId, operationId: parent.request.workflow.nextOperationId });
    } catch (error) { if (error instanceof GitWorkflowClosed) throw error; throw new GitResultUnknown(); }
    if (!known) throw new GitResultUnknown();
    result = known;
  }
  adoptChild(parent, result);
  return result;
}
export const gitOperations = {
  entries,
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
  snapshot(taskId: string) { try { return JSON.stringify(entries(taskId).map(item => ({ ...item, sending: sending.has(item.request.operationId) }))); } catch (error) { return JSON.stringify({ error: String(error) }); } },
  async run(input: Omit<GitRequest, "operationId" | "snapshotId" | "stageAll"> & { snapshotId?: string | null; stageAll?: boolean }, intent: PendingGit["intent"] = input.action, options: { operationId?: string; nextOperationId?: string; committed?: boolean; retryOf?: string } = {}): Promise<boolean> {
    const existing = entries(input.taskId).filter(item => item.request.rootId === input.rootId);
    const alreadySending = existing.find(item => item.request.operationId === options.operationId);
    if (alreadySending && sameRequest(alreadySending.request, { ...input, operationId: options.operationId!, stageAll: input.stageAll ?? false, snapshotId: input.snapshotId ?? null })) return send(alreadySending);
    const previous = existing.length === 1 ? existing[0] : null;
    const continuation = input.action === "push" && previous?.intent === "commitPush" && previous.request.action === "commit"
      && previous.receipt?.outcome.status === "succeeded" && previous.receipt.outcome.completed;
    const retry = input.action === "push" && previous?.request.action === "push" && previous.committed
      && previous.receipt?.outcome.status === "failed" && options.retryOf === previous.request.operationId
      && !!previous.retryOperationId && options.operationId === previous.retryOperationId;
    if (existing.length && !continuation && !retry) throw new GitResultUnknown();
    if (continuation && previous!.request.workflow) {
      const result = await advanceWorkflow(previous!); return settle(fromReceipt(result), result);
    }
    const nextOperationId = intent === "commitPush" ? options.nextOperationId ?? crypto.randomUUID() : undefined;
    const value: PendingGit = {
      request: { ...input, operationId: continuation ? previous!.nextOperationId! : options.operationId ?? crypto.randomUUID(), snapshotId: input.snapshotId ?? null, stageAll: input.stageAll ?? false, ...(nextOperationId ? { workflow: { nextOperationId, parentOperationId: null } } : {}) },
      intent, ...(intent === "commitPush" ? { nextOperationId } : {}), ...(continuation || options.committed ? { committed: true } : {}),
      ...(input.action === "push" ? { retryOperationId: crypto.randomUUID() } : {}),
    };
    if (!value.request.operationId) throw new Error("后续推送标识缺失，请先核实提交记录");
    // Unique per-operation keys: another window cannot overwrite this request.
    // The push ID was reserved before the commit, so two recovery views cannot
    // turn the same continuation into two independent push operations.
    store(value);
    if (continuation || retry) remove(previous!);
    return send(value);
  },
  async check(value: PendingGit): Promise<GitReceipt | null> {
    const result = await receipt(value);
    if (result?.closed) { remove(value); return result; }
    if (result?.request.workflow && localStorage.getItem(key(value.request.operationId))) {
      const child = await invoke<GitReceipt | null>("core_review_git_operation", { taskId: value.request.taskId, operationId: result.request.workflow.nextOperationId });
      if (child && localStorage.getItem(key(value.request.operationId))) { adoptChild(value, child); return child; }
    }
    // A late status reply must not recreate a record completed by another view.
    if (localStorage.getItem(key(value.request.operationId))) store({ ...value, receipt: result });
    return result;
  },
  async resend(value: PendingGit) {
    const result = await receipt(value);
    const completed = result ? settle(value, result) : await send(value); // exact original ID/payload
    if (!completed) store({ ...value, receipt: result ?? { request: value.request, outcome: { status: "succeeded", completed: false } } });
    return completed;
  },
  async continuePush(value: PendingGit) {
    const result = await receipt(value);
    if (value.intent !== "commitPush" || value.request.action !== "commit" || result?.outcome.status !== "succeeded" || !result.outcome.completed) throw new GitResultUnknown();
    if (localStorage.getItem(key(value.request.operationId))) store({ ...value, receipt: result });
    if (result.request.workflow) { const child = await advanceWorkflow(fromReceipt(result)); return settle(fromReceipt(child), child); }
    if (!value.nextOperationId) throw new Error("后续推送标识缺失，请先核实提交记录");
    return this.run({ taskId: value.request.taskId, rootId: value.request.rootId, action: "push", message: "" }, "push", { operationId: value.nextOperationId, committed: true });
  },
  async retryPush(value: PendingGit) {
    const result = await receipt(value);
    if (!value.committed || value.request.action !== "push" || result?.outcome.status !== "failed") throw new GitResultUnknown();
    if (result.request.workflow) { const child = await advanceWorkflow(fromReceipt(result)); return settle(fromReceipt(child), child); }
    if (!value.retryOperationId) throw new Error("重试推送标识缺失，请先核实仓库记录");
    if (localStorage.getItem(key(value.request.operationId))) store({ ...value, receipt: result });
    const { operationId, ...input } = value.request;
    return this.run(input, "push", { operationId: value.retryOperationId, committed: true, retryOf: operationId });
  },
  async advance(taskId: string, operationId: string) {
    let parent: GitReceipt | null;
    try { parent = await invoke<GitReceipt | null>("core_review_git_operation", { taskId, operationId }); }
    catch { throw new GitResultUnknown(); }
    if (!parent || parent.request.taskId !== taskId || parent.request.operationId !== operationId || !parent.request.workflow) throw new GitResultUnknown();
    const child = await advanceWorkflow(fromReceipt(parent));
    return settle(fromReceipt(child), child);
  },
  async advanceFromReceipt(parent: GitReceipt) {
    const child = await advanceWorkflow(fromReceipt(parent));
    if (child.outcome.status === "succeeded") settle(fromReceipt(child), child);
    return child;
  },
  async acknowledge(value: PendingGit) {
    let result: GitReceipt | null;
    try { result = await invoke<GitReceipt>("core_review_git_close", { taskId: value.request.taskId, operationId: value.request.operationId }); }
    catch { result = await receipt(value); }
    if (!result?.closed || !sameRequest(result.request, value.request)) throw new Error("尚未确认后台已结束此次操作，请核实后重试。");
    remove(value);
  },
};
