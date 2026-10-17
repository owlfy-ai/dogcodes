// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { DeleteConversation } from "./DeleteConversation";
import type { TaskSummary } from "@dogcodes/protocol";
const task: TaskSummary = {taskId:"tsk_delete",projectId:"prj_one",title:"待删除会话",preferredExecutor:{providerId:"codex",installationId:null},activePrimaryRun:null,streamVersion:3};
let root: Root, host: HTMLDivElement;
const remove = vi.fn();
const button = (text: string) => [...document.querySelectorAll("button")].find(b => b.textContent === text)!;
const click = async (element: HTMLElement) => act(async () => element.click());
async function render(blocked = false) { await act(async () => root.render(<DeleteConversation task={task} blocked={blocked} onDelete={remove} />)); }
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  remove.mockReset().mockResolvedValue(undefined);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
it("requires confirmation, focuses the dialog, and cancels without deleting", async () => {
  await render(); await click(host.querySelector("button")!);
  expect(document.querySelector("dialog")?.open).toBe(true);
  expect(document.activeElement).toBe(document.querySelector("h2"));
  expect(document.querySelector("dialog")?.textContent).toContain("项目代码和 Agent 原生会话不会删除");
  expect(document.querySelector("dialog")?.textContent).toContain("无法恢复");
  expect(document.querySelector("dialog")?.textContent).toContain("历史 Diff 和快照");
  await click(button("取消")); expect(remove).not.toHaveBeenCalled();
  expect(document.querySelector("dialog")).toBeNull(); expect(document.activeElement).toBe(host.querySelector("button"));
});
it("blocks active work even if it starts after opening confirmation", async () => {
  await render(true); expect(host.querySelector("button")!.disabled).toBe(true);
  await render(); await click(host.querySelector("button")!); await render(true);
  expect(button("彻底删除").disabled).toBe(true);
  await click(button("彻底删除")); expect(remove).not.toHaveBeenCalled();
});
it("keeps a failed deletion retryable and prevents duplicate submissions", async () => {
  remove.mockRejectedValueOnce(new Error("conflict"));
  await render(); await click(host.querySelector("button")!); await click(button("彻底删除"));
  expect(document.querySelector('[role="alert"]')).not.toBeNull();
  let finish!: () => void; remove.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
  await click(button("彻底删除")); await click(button("正在删除…"));
  expect(remove).toHaveBeenCalledTimes(2); expect(button("取消").disabled).toBe(true);
  await act(async () => finish()); expect(document.querySelector("dialog")).toBeNull();
  expect(remove).toHaveBeenLastCalledWith(task);
});
