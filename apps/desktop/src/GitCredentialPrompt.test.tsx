// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { I18nProvider } from "./i18n";
import { GitCredentialPrompt, type GitCredentialApi } from "./GitCredentialPrompt";
import { gitOperations } from "./gitOperations";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

let root: Root, host: HTMLDivElement;
const prompt = (secret: boolean) => ({ id: 7, text: secret ? "Password for 'https://x': " : "Username for 'https://x': ", secret });
const render = async (api: GitCredentialApi) => {
  await act(async () => root.render(<I18nProvider language="en"><GitCredentialPrompt taskId="task" api={api} /></I18nProvider>));
};
const running = (value: boolean) => vi.spyOn(gitOperations, "snapshot").mockReturnValue(value ? '[{"sending":true}]' : "[]");
const input = () => host.querySelector<HTMLInputElement>("dialog input")!;
const button = (text: string) => [...host.querySelectorAll("button")].find(item => item.textContent === text)!;
const type = async (value: string) => act(async () => {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input(), value);
  input().dispatchEvent(new Event("input", { bubbles: true }));
});

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); });

it("does not ask the service unless a Git operation is running", async () => {
  running(false);
  const api: GitCredentialApi = { current: vi.fn(async () => prompt(false)), answer: vi.fn(async () => {}) };
  await render(api);
  expect(api.current).not.toHaveBeenCalled();
  expect(host.querySelector("dialog")).toBeNull();
});

it("shows Git's question, hides a password and sends the answer once", async () => {
  running(true);
  let current: ReturnType<typeof prompt> | null = prompt(true);
  const api: GitCredentialApi = { current: vi.fn(async () => current), answer: vi.fn(async () => { current = null; }) };
  await render(api);
  expect(host.querySelector("dialog")?.textContent).toContain("Password for 'https://x'");
  expect(input().type).toBe("password");
  await type("hunter2");
  await act(async () => host.querySelector("form")!.requestSubmit());
  expect(api.answer).toHaveBeenCalledExactlyOnceWith("task", 7, "hunter2");
  expect(host.querySelector("dialog")).toBeNull();
});

it("shows a username as it is typed, and cancelling declines", async () => {
  running(true);
  const api: GitCredentialApi = { current: vi.fn(async () => prompt(false)), answer: vi.fn(async () => {}) };
  await render(api);
  expect(input().type).toBe("text");
  await act(async () => button("Cancel").click());
  expect(api.answer).toHaveBeenCalledExactlyOnceWith("task", 7, null);
});

it("keeps the dialog when the answer can't be delivered", async () => {
  running(true);
  const api: GitCredentialApi = { current: vi.fn(async () => prompt(true)), answer: vi.fn(async () => { throw new Error("gone"); }) };
  await render(api);
  await act(async () => host.querySelector("form")!.requestSubmit());
  expect(host.querySelector("dialog [role=alert]")).not.toBeNull();
  expect(button("Continue").disabled).toBe(false);
});
