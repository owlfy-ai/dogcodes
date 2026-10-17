// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";
let root: Root, host: HTMLDivElement, client: DemoCoreClient;
const click = async (node: HTMLElement) => act(async () => node.click());
const title = () => host.querySelector('.task-heading h1')?.textContent;
const confirm = () => [...document.querySelectorAll<HTMLButtonElement>('dialog button')].find(button => button.textContent === '彻底删除')!;
beforeEach(async () => {
  localStorage.clear();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  client = new DemoCoreClient();
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
  await act(async () => root.render(<App client={client} />));
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it('permanently deletes an archived conversation without offering restoration', async () => {
  const original = (await client.bootstrap()).tasks[0];
  expect(title()).toBe(original.title);
  expect(host.querySelector('[aria-label="删除会话"]')).toBeNull();
  await click(host.querySelector<HTMLButtonElement>('[aria-label="归档会话"]')!);
  await click(host.querySelector<HTMLElement>('.history-archived button[aria-expanded]')!);
  await click(host.querySelector<HTMLButtonElement>('[aria-label="删除会话"]')!);
  await click(confirm());
  expect((await client.bootstrap()).tasks).toHaveLength(0);
  expect(host.querySelector('.task[aria-current="page"]')).toBeNull();
  expect(host.querySelector('textarea')).not.toBeNull();
  expect(host.querySelector('.history-deleted')).toBeNull();
  expect(host.textContent).not.toContain('最近删除');
  expect((await client.bootstrap()).deletedTasks).toEqual([]);
  expect(client).not.toHaveProperty('restoreTask');
  await expect(client.timeline(original.taskId)).rejects.toThrow();

});
it('removes an externally deleted current task from navigation and switches to the other task', async () => {
  const original = (await client.bootstrap()).tasks[0];
  await act(async () => { await client.createTask(original.projectId, 'Other', original.preferredExecutor); });
  await act(async () => { await client.deleteTask(original); });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 100)); });
  expect(title()).toBe('Other');
  expect(host.querySelector<HTMLButtonElement>('[aria-label="上一个会话"]')!.disabled).toBe(true);
  expect(host.querySelector(`.task[data-tooltip="${original.title}"]`)).toBeNull();
});
