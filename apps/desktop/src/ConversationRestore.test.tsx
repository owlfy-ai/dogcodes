// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { TaskSummary } from "@dogcodes/protocol";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";

const storageKey = "dogcodes.conversation.lastViewed.v1";
let root: Root, host: HTMLDivElement, client: DemoCoreClient;
let first: TaskSummary, second: TaskSummary;
const title = () => host.querySelector(".task-heading h1")?.textContent;
const click = async (node: Element) => act(async () => { (node as HTMLElement).click(); });
const open = async (task: TaskSummary) => click(host.querySelector(`.task[data-tooltip="${task.title}"]`)!);
const mount = async () => act(async () => root.render(<App client={client} />));
const reopen = async () => {
  await act(async () => root.unmount());
  root = createRoot(host);
  await mount();
};

beforeEach(async () => {
  localStorage.clear();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
  client = new DemoCoreClient();
  first = (await client.bootstrap()).tasks[0];
  const id = await client.createTask(first.projectId, "最后查看的会话", first.preferredExecutor);
  second = (await client.bootstrap()).tasks.find(task => task.taskId === id)!;
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
});

it("reopens the last viewed conversation and loads only its timeline", async () => {
  await mount();
  expect(title()).toBe(first.title);
  await open(second);
  const timeline = vi.spyOn(client, "timeline");
  await reopen();
  expect(title()).toBe(second.title);
  expect(timeline).toHaveBeenCalledWith(second.taskId);
  expect(timeline).not.toHaveBeenCalledWith(first.taskId);
});

it.each(["上一个会话", "下一个会话"])("remembers selection made with %s", async label => {
  await mount();
  await open(second);
  await click(host.querySelector('[aria-label="上一个会话"]')!);
  if (label === "下一个会话") await click(host.querySelector('[aria-label="下一个会话"]')!);
  await reopen();
  expect(title()).toBe(label === "上一个会话" ? first.title : second.title);
});

it("keeps the last viewed conversation when closing on an unsent draft", async () => {
  await mount();
  await open(second);
  await click([...host.querySelectorAll("button")].find(button => button.textContent?.includes("新建对话"))!);
  expect(host.querySelector(".welcome-stage")).not.toBeNull();
  await reopen();
  expect(title()).toBe(second.title);
});

it("falls back when the remembered conversation was deleted", async () => {
  localStorage.setItem(storageKey, "tsk_deleted");
  await mount();
  expect(title()).toBe(first.title);
  expect(localStorage.getItem(storageKey)).toBe(first.taskId);
});

it("opens an empty draft when no conversations remain", async () => {
  localStorage.setItem(storageKey, second.taskId);
  await client.deleteTask(first);
  await client.deleteTask(second);
  await mount();
  expect(host.querySelector(".welcome-stage")).not.toBeNull();
  expect(host.querySelector("textarea")).not.toBeNull();
});

it("retains the remembered conversation through a failed bootstrap and retry", async () => {
  localStorage.setItem(storageKey, second.taskId);
  vi.spyOn(client, "bootstrap").mockRejectedValueOnce(new Error("temporarily unavailable"));
  await mount();
  expect(localStorage.getItem(storageKey)).toBe(second.taskId);
  await click(host.querySelector(".startup-actions button")!);
  expect(title()).toBe(second.title);
});

it("still opens and switches conversations when local storage is unavailable", async () => {
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("storage disabled"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage disabled"); });
  await mount();
  expect(title()).toBe(first.title);
  await open(second);
  expect(title()).toBe(second.title);
});
