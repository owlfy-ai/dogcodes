// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { SharedAgentPreferencesGate } from "./SharedAgentPreferencesGate";
import { AGENT_PREFERENCES_KEY, useAgentPreferences } from "./agentPreferences";

const api = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn(), subscribe: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("./tauriClient", () => ({ TauriCoreClient: class {
  agentPreferences = api.read;
  setAgentPreferences = api.write;
  subscribe = api.subscribe;
} }));

let root: Root;
let host: HTMLDivElement;
const legacy = { order: ["kimi", "codex"], disabled: ["codex"] };
function Child() { const state = useAgentPreferences(); return <output>{JSON.stringify(state)}</output>; }
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  localStorage.setItem(AGENT_PREFERENCES_KEY, JSON.stringify(legacy));
  api.read.mockReset(); api.write.mockReset(); api.subscribe.mockReset();
  api.read.mockResolvedValue({ revision: 0, lastSequence: 0, preferences: { order: [], disabled: [] } });
  api.subscribe.mockResolvedValue(() => {});
  api.write.mockResolvedValue(undefined);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); vi.useRealTimers(); vi.unstubAllGlobals(); });

it("delays the loading indicator and retains a displayed loader for its minimum duration", async () => {
  vi.useFakeTimers();
  let resolve!: (value: unknown) => void;
  api.read.mockImplementationOnce(() => new Promise(yes => { resolve = yes; }));
  await act(async () => root.render(<SharedAgentPreferencesGate><Child /></SharedAgentPreferencesGate>));
  expect(host.querySelector(".activity-loader")).toBeNull();
  await act(async () => { await vi.advanceTimersByTimeAsync(300); });
  expect(host.querySelector(".activity-loader")).not.toBeNull();
  await act(async () => resolve({ revision: 1, lastSequence: 1, preferences: legacy }));
  expect(host.querySelector("output")).toBeNull();
  await act(async () => { await vi.advanceTimersByTimeAsync(400); });
  expect(host.querySelector("output")?.textContent).toBe(JSON.stringify(legacy));
});

it("shows a read failure with a retry instead of exposing stale local preferences", async () => {
  api.read.mockRejectedValueOnce(new Error("service disconnected"));
  await act(async () => root.render(<SharedAgentPreferencesGate><Child /></SharedAgentPreferencesGate>));
  expect(host.querySelector("output")).toBeNull();
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("暂时无法同步");
  expect(host.querySelector(".activity-loader")).toBeNull();
  api.read.mockResolvedValue({ revision: 1, lastSequence: 1, preferences: legacy });
  await act(async () => host.querySelector("button")!.click());
  expect(host.querySelector("output")?.textContent).toBe(JSON.stringify(legacy));
});

it("asks before importing local settings and preserves the original local copy", async () => {
  await act(async () => root.render(<SharedAgentPreferencesGate><Child /></SharedAgentPreferencesGate>));
  expect(host.textContent).toContain("保留已有 Agent 设置");
  expect(host.querySelector("output")).toBeNull();
  expect(api.write).not.toHaveBeenCalled();
  const button = [...host.querySelectorAll("button")].find(button => button.textContent === "保留并同步现有设置")!;
  await act(async () => button.click());
  expect(api.write).toHaveBeenCalledWith(legacy, 0, expect.stringMatching(/^cmd_/));
  expect(host.querySelector("output")?.textContent).toBe(JSON.stringify(legacy));
  expect(localStorage.getItem(AGENT_PREFERENCES_KEY)).toBe(JSON.stringify(legacy));
});

it("uses existing host settings without importing a different window's local copy", async () => {
  const preferences = { order: ["cursor"], disabled: ["kimi"] };
  api.read.mockResolvedValue({ revision: 7, lastSequence: 20, preferences });
  await act(async () => root.render(<SharedAgentPreferencesGate><Child /></SharedAgentPreferencesGate>));
  expect(host.querySelector("output")?.textContent).toBe(JSON.stringify(preferences));
  expect(api.write).not.toHaveBeenCalled();
  expect(localStorage.getItem(AGENT_PREFERENCES_KEY)).toBe(JSON.stringify(legacy));
});

it("does not trap a corrupt local import in unknown-result retry and keeps the backup", async () => {
  const corrupt = JSON.stringify({ order: ["invalid\nprovider"], disabled: [] });
  localStorage.setItem(AGENT_PREFERENCES_KEY, corrupt);
  await act(async () => root.render(<SharedAgentPreferencesGate><Child /></SharedAgentPreferencesGate>));
  await act(async () => [...host.querySelectorAll("button")].find(button => button.textContent === "保留并同步现有设置")!.click());
  expect(api.write).not.toHaveBeenCalled();
  expect(host.textContent).toContain("原有 Agent 设置包含无效内容");
  const defaults = [...host.querySelectorAll("button")].find(button => button.textContent === "使用默认设置")!;
  expect(defaults.disabled).toBe(false);
  await act(async () => defaults.click());
  expect(host.querySelector("output")?.textContent).toBe(JSON.stringify({ order: [], disabled: [] }));
  expect(localStorage.getItem(AGENT_PREFERENCES_KEY)).toBe(corrupt);
});

it("keeps an unconfirmed import visible and retries its original operation", async () => {
  api.write.mockRejectedValueOnce(new Error("lost reply"));
  await act(async () => root.render(<SharedAgentPreferencesGate><Child /></SharedAgentPreferencesGate>));
  const button = [...host.querySelectorAll("button")].find(button => button.textContent === "保留并同步现有设置")!;
  await act(async () => button.click());
  const request = api.write.mock.calls[0];
  expect(host.querySelector("output")).toBeNull();
  expect(button.disabled).toBe(true);
  expect(host.textContent).toContain("保存结果尚未确认");
  await act(async () => [...host.querySelectorAll("button")].find(button => button.textContent === "重新核实")!.click());
  expect(api.write.mock.calls[1]).toEqual(request);
  expect(host.querySelector("output")?.textContent).toBe(JSON.stringify(legacy));
});
