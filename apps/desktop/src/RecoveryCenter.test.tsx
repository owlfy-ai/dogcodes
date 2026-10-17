// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient } from "@dogcodes/protocol";
import { RecoveryCenter } from "./RecoveryCenter";
let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
it.each([false, true])("prepares replacement without submitting a prompt (stale=%s)", async (stale) => {
  const prepareSessionRebuild = stale ? vi.fn().mockRejectedValue(new Error("conflict")) : vi.fn().mockResolvedValue(undefined);
  const submitTurn = vi.fn();
  const recoverySnapshot = vi.fn().mockResolvedValue({ generatedAt: new Date().toISOString(), truncated: false, items: [{ code: "session_unavailable", taskId: "tsk_test", expectedStreamVersion: 7, canCancel: false }] });
  const client = { recoverySnapshot, prepareSessionRebuild, submitTurn } as unknown as CoreClient;
  await act(async () => root.render(<RecoveryCenter client={client} onBack={() => {}} onOpenTask={() => {}} />));
  const button = [...container.querySelectorAll("button")].find(button => button.textContent === "重建会话并保留历史")!;
  await act(async () => button.click());
  expect(prepareSessionRebuild).toHaveBeenCalledWith("tsk_test", 7);
  expect(submitTurn).not.toHaveBeenCalled();
  expect(container.textContent).toContain(stale ? "状态可能已变化" : "不会自动重发旧指令");
});

it("offers returning to the conversation without asking users to attest external execution", async () => {
  const cancelTurn = vi.fn(); const submitTurn = vi.fn(); const onOpenTask = vi.fn();
  const recoverySnapshot = vi.fn().mockResolvedValue({ generatedAt: new Date().toISOString(), items: [{code:"run_ownership_unknown", taskId:"tsk_test", runId:"run_old", expectedStreamVersion:8, canCancel:false}] });
  const client = {recoverySnapshot, cancelTurn, submitTurn} as unknown as CoreClient;
  await act(async () => root.render(<RecoveryCenter client={client} onBack={() => {}} onOpenTask={onOpenTask} />));
  expect(container.querySelector('input[type="checkbox"]')).toBeNull();
  expect(container.textContent).not.toContain("确认结束");
  expect(container.textContent).not.toContain("状态待确认");
  const button = [...container.querySelectorAll("button")].find(b => b.textContent === "返回会话处理")!;
  await act(async () => button.click());
  expect(onOpenTask).toHaveBeenCalledWith("tsk_test");
  expect(cancelTurn).not.toHaveBeenCalled(); expect(submitTurn).not.toHaveBeenCalled();
});
