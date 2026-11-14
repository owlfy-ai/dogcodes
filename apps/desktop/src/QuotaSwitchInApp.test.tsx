// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";

let root: Root, host: HTMLDivElement, client: DemoCoreClient;
const settle = () => act(async () => { await vi.advanceTimersByTimeAsync(200); });

beforeEach(async () => {
  localStorage.clear();
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
  client = new DemoCoreClient();
  const views = await client.quotaSnapshots();
  views[0].quota!.windows[0].usedPercent = 100;
  vi.spyOn(client, "quotaSnapshots").mockImplementation(async () => {
    const copy = structuredClone(views);
    copy[0].quota!.staleAfterUnixMs = Date.now() + 60_000;
    return copy;
  });
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<App client={client} />));
  await settle();
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("lets a conversation move on from an agent that's out of quota, keeping what was typed", async () => {
  const notice = () => host.querySelector(".quota-switch-notice");
  expect(notice()?.textContent).toContain("Codex");
  expect(host.querySelector<HTMLButtonElement>(".composer-submit")!.disabled).toBe(true);
  const input = host.querySelector<HTMLTextAreaElement>("form.composer textarea")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(input, "keep me");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => notice()!.querySelector("button")!.click());
  await settle();
  expect(notice()).toBeNull();
  expect(host.querySelector<HTMLTextAreaElement>("form.composer textarea")!.value).toBe("keep me");
  expect(host.querySelector<HTMLButtonElement>(".composer-submit")!.disabled).toBe(false);
  expect(host.querySelector(".agent-picker summary")?.getAttribute("data-tooltip")).not.toContain("Codex");
});

it("picks up a turn that ran out of quota on the agent the person switches to", async () => {
  const { bootstrap } = client;
  const task = (await bootstrap.call(client)).tasks[0];
  const { timeline } = client;
  vi.spyOn(client, "timeline").mockImplementation(async (taskId) => {
    const items = await timeline.call(client, taskId);
    return taskId !== task.taskId ? items : [...items, { attachments: [], itemId: "quota", taskId, turnId: "trn_quota",
      kind: "quota.exhausted", text: "backendText.quotaExhausted", providerId: "codex", sequence: 99, occurredAt: new Date().toISOString() }];
  });
  const submitted = vi.spyOn(client, "submitTurn");
  await act(async () => root.unmount());
  root = createRoot(host);
  await act(async () => root.render(<App client={client} />));
  await settle();
  const button = host.querySelector<HTMLButtonElement>(".quota-switch-notice button")!;
  expect(button.dataset.tooltip).toContain("未做完的任务");
  await act(async () => button.click());
  await settle();
  expect(submitted).toHaveBeenCalledTimes(1);
  const [sentTask, prompt] = submitted.mock.calls[0];
  expect(prompt).toBe("Continue the task from where it stopped.");
  expect(sentTask.preferredExecutor.providerId).not.toBe("codex");
  // The timeline shows the request in the person's language.
  expect(host.textContent).toContain("从中断的地方继续完成任务。");
});
