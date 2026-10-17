import type { CoreClient, ForkReceipt, ForkRequest, TaskId } from "@dogcodes/protocol";

const prefix = "dogcodes:pending-fork:";
const listeners = new Set<() => void>();
const key = (task: string, turn: string) => `${prefix}${task}:${turn}`;
function emit() { listeners.forEach(listener => listener()); }
function matches(a: ForkRequest, b: ForkRequest) {
  return a.taskId === b.taskId && a.turnId === b.turnId && a.newTaskId === b.newTaskId && a.environment === b.environment;
}
function read(task: string, turn: string): ForkRequest | null {
  const raw = localStorage.getItem(key(task, turn));
  if (!raw) return null;
  const value = JSON.parse(raw) as ForkRequest;
  if (!value || value.taskId !== task || value.turnId !== turn || typeof value.newTaskId !== "string" || !value.newTaskId || !["workspace", "worktree"].includes(value.environment)) throw new Error("待核实的 Fork 请求记录损坏，请保留记录并检查本机存储。");
  return value;
}
function forget(request: ForkRequest) {
  if (read(request.taskId, request.turnId)?.newTaskId === request.newTaskId) {
    localStorage.removeItem(key(request.taskId, request.turnId)); emit();
  }
}
class ForkSourceDeleted extends Error {}
async function check(client: CoreClient, request: ForkRequest): Promise<ForkReceipt | null> {
  if (client.taskDeletion) {
    const deleted = await client.taskDeletion(request.taskId);
    if (deleted) {
      if (deleted.taskId !== request.taskId) throw new Error("会话删除证明不匹配，已保留 Fork 请求。");
      forget(request);
      throw new ForkSourceDeleted("源会话已永久删除，Fork 恢复已结束；现有 Worktree 已保留，未重新创建。");
    }
  }
  if (!client.forkReceipt) throw new Error("当前客户端不支持查询 Fork 结果，请更新后核实。");
  const receipt = await client.forkReceipt(request.taskId, request.newTaskId);
  if (receipt && (!receipt.request || !matches(receipt.request, request))) throw new Error("Fork 回执与原请求不一致，未重复执行。");
  return receipt;
}
function available(receipt: ForkReceipt): TaskId {
  if (!receipt.completed) throw new Error("原 Fork 尚无完成回执，请核实后重试原请求。");
  if (!receipt.taskAvailable) throw new Error("原 Fork 已创建，但目标会话已不可用；未重新创建。");
  return receipt.request.newTaskId;
}
async function send(client: CoreClient, request: ForkRequest): Promise<TaskId> {
  if (!client.forkTask) throw new Error("当前客户端不支持 Fork。");
  const previous = read(request.taskId, request.turnId);
  if (previous && !matches(previous, request)) throw new Error("请先核实原 Fork 请求，不能更换目标或环境重试。");
  localStorage.setItem(key(request.taskId, request.turnId), JSON.stringify(request)); emit();
  try {
    const id = await client.forkTask(request);
    if (id !== request.newTaskId) throw new Error("后台返回了其他 Fork 会话，请核实原请求。");
    return id;
  } catch (cause) {
    let receipt: ForkReceipt | null = null;
    try { receipt = await check(client, request); } catch (error) { if (error instanceof ForkSourceDeleted) throw error; /* Preserve pending intent for explicit recovery. */ }
    if (receipt?.completed) return available(receipt);
    throw new Error(`${cause instanceof Error ? cause.message : String(cause)}；原 Fork 请求已保留，请先核实结果。`);
  }
}
export const forkOperations = {
  read, forget, check, send, available,
  async resend(client: CoreClient, request: ForkRequest) {
    const receipt = await check(client, request);
    if (receipt?.status === "running") throw new Error("原 Fork 仍在后台执行，请等待结果；未重复创建。");
    return receipt?.completed ? available(receipt) : send(client, request);
  },
  snapshot(task: string, turn: string) { try { return JSON.stringify(read(task, turn)); } catch (error) { return JSON.stringify({ error: String(error) }); } },
  subscribe(listener: () => void) { listeners.add(listener); window.addEventListener("storage", listener); return () => { listeners.delete(listener); window.removeEventListener("storage", listener); }; },
};
