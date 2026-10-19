// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { AGENT_PREFERENCES_KEY, enabledAgents, moveAgent, orderAgents, readAgentPreferences, setAgentEnabled, useAgentPreferences } from "./agentPreferences";
import { AgentPickerMenu } from "./AgentPickerMenu";
import { AgentManagementControls, AGENT_DRAG_TYPE } from "./AgentManagementControls";
import { QuotaPanel } from "./App";
import type { InstallationSummary } from "@dogcodes/protocol";
import { readyAgentInstallations } from "./agentOptions";
let host: HTMLDivElement, root: Root;
const installations: InstallationSummary[] = [
  { providerId: "codex", installationId: "ins_codex", version: "1.0", status: "ready" },
  { providerId: "kimi", installationId: "ins_kimi", version: "1.0", status: "ready" },
  { providerId: "cursor", installationId: "ins_cursor", version: "1.0", status: "broken" },
].map(item => ({ executable: "fixture", fingerprint: "fixture", versionText: "1.0", diagnostic: null, probeElapsedMillis: 0, probedAt: "2026-10-01", ...item, installationId: item.installationId as `ins_${string}`, status: item.status as InstallationSummary["status"] }));
function Fixture() {
  useAgentPreferences();
  return <><QuotaPanel views={[]} installations={installations} management />
    <AgentPickerMenu options={readyAgentInstallations(installations).map(executor => ({ key: executor.providerId, executor }))} selectedKey="" title="选择 Agent" label="测试 Agent 列表" onSelect={() => {}} /></>;
}
const menuNames = () => [...document.querySelectorAll('.agent-picker-option')].map(node => node.getAttribute('aria-label'));
const cardNames = () => [...host.querySelectorAll('.agent-management-card')].map(node => node.getAttribute('aria-label'));
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); localStorage.clear(); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); vi.unstubAllGlobals(); });
it("keeps preference order and disabled state by provider across replacement installs", () => {
  setAgentEnabled("codex", false);
  moveAgent("kimi", "codex", ["codex", "kimi", "cursor"]);
  const preferences = readAgentPreferences();
  expect(preferences.order).toEqual(["kimi", "codex", "cursor"]);
  expect(JSON.parse(localStorage.getItem(AGENT_PREFERENCES_KEY)!)).toEqual(preferences);
  const replacement = [...installations, { ...installations[0], installationId: "ins_replaced" as const }];
  expect(enabledAgents(readyAgentInstallations(replacement), item => item.providerId, preferences).map(item => item.providerId)).toEqual(["kimi"]);
  setAgentEnabled("codex", true);
  expect(readAgentPreferences().order).toEqual(preferences.order);
});
it("normalizes malformed preferences and keeps newly discovered providers", () => {
  localStorage.setItem(AGENT_PREFERENCES_KEY, '{"order":["kimi","kimi",3],"disabled":"codex"}');
  expect(readAgentPreferences()).toEqual({order:["kimi"],disabled:[]});
  expect(orderAgents(["new-agent", "codex", "kimi"], id => id, readAgentPreferences())).toEqual(["kimi", "codex", "new-agent"]);
  localStorage.setItem(AGENT_PREFERENCES_KEY, "broken");
  expect(readAgentPreferences()).toEqual({order:[],disabled:[]});
});
it("retains pre-rename preferences while letting an explicit current value take precedence", () => {
  const key = "codesheepdog.agents.preferences.v1";
  const backup = JSON.stringify({ order: ["kimi"], disabled: ["codex"] });
  localStorage.setItem(key, backup);
  expect(readAgentPreferences()).toEqual({ order: ["kimi"], disabled: ["codex"] });
  localStorage.setItem(AGENT_PREFERENCES_KEY, JSON.stringify({ order: [], disabled: [] }));
  expect(readAgentPreferences()).toEqual({ order: [], disabled: [] });
  expect(localStorage.getItem(key)).toBe(backup);
});
it("offers native login in management only for installed agents with missing authentication", async () => {
  await act(async () => root.render(<QuotaPanel management views={[]} installations={[
    { ...installations[0], status: "not_configured", diagnostic: "authentication_failed" }, installations[1], installations[2],
  ]} />));
  expect([...host.querySelectorAll(".agent-login-button")].map(button => button.getAttribute("aria-label"))).toEqual(["Codex：去登录"]);
  await act(async () => root.render(<QuotaPanel management installations={installations} views={[
    { providerId: "kimi", installationId: "ins_kimi", quota: null, diagnostic: "quota_login_required" },
  ]} />));
  expect([...host.querySelectorAll(".agent-login-button")].map(button => button.getAttribute("aria-label"))).toEqual(["Kimi Code：去登录"]);
});
it("re-detects only the uninstalled agents from management", async () => {
  let finish = () => {};
  const onDetect = vi.fn((_ids: string[]) => new Promise<void>(resolve => { finish = resolve; }));
  await act(async () => root.render(<QuotaPanel management views={[]} installations={installations} onDetect={onDetect} />));
  const button = [...host.querySelectorAll<HTMLButtonElement>(".agent-uninstalled button")].find(item => item.textContent === "重新检测")!;
  await act(async () => button.click());
  expect(onDetect).toHaveBeenCalledTimes(1);
  const scanned = onDetect.mock.calls[0][0];
  expect(scanned).toContain("claude-code");
  expect(scanned).not.toContain("codex");
  expect(button.disabled).toBe(true);
  await act(async () => finish());
  expect(button.disabled).toBe(false);
});
it("updates open menus immediately and restores a disabled Agent in its saved position", async () => {
  await act(async () => root.render(<Fixture />));
  expect(menuNames()).toEqual(["Codex", "Kimi"]);
  // Known but uninstalled Agents are listed compactly with an install link, never as sortable cards or menu candidates.
  expect(cardNames()).toEqual(["Codex", "Cursor", "Kimi"]);
  const missing = host.querySelector('.agent-uninstalled li[aria-label="Claude Code"]')!;
  expect(missing.querySelector('[role="switch"], .agent-drag-handle')).toBeNull();
  expect(missing.querySelector('a')!.getAttribute("aria-label")).toBe("Claude Code：前往官网安装");
  await act(async () => host.querySelector<HTMLButtonElement>('[role="switch"][aria-label="启用 Codex"]')!.click());
  expect(menuNames()).toEqual(["Kimi"]);
  expect(host.querySelector('[aria-label="启用 Codex"]')!.getAttribute('aria-checked')).toBe('false');
  await act(async () => setAgentEnabled("kimi", false));
  expect(menuNames()).toEqual([]);
  expect(document.body.textContent).toContain("暂无可用 Agent");
  await act(async () => setAgentEnabled("codex", true));
  expect(menuNames()).toEqual(["Codex"]);
});
it("persists drag sorting and applies it to the shared picker", async () => {
  await act(async () => root.render(<Fixture />));
  const data = new Map<string,string>();
  const transfer = { types: [AGENT_DRAG_TYPE], setData: (key:string,value:string) => data.set(key,value), getData: (key:string) => data.get(key), effectAllowed: "", dropEffect: "" };
  const dispatch = async (node: Element, type: string) => act(async () => { const event = new Event(type, { bubbles: true, cancelable: true }); Object.defineProperty(event, "dataTransfer", { value: transfer }); node.dispatchEvent(event); });
  await dispatch(host.querySelector('[aria-label="调整 Kimi 排序"]')!, "dragstart");
  const target = host.querySelector('.agent-management-card[aria-label="Codex"]')!;
  await dispatch(target, "dragover");
  expect(target.getAttribute('data-drop-target')).toBe('true');
  await dispatch(target, "drop");
  expect(cardNames()[0]).toBe("Kimi");
  expect(menuNames()).toEqual(["Kimi", "Codex"]);
  await act(async () => root.render(<Fixture key="remount" />));
  expect(cardNames()[0]).toBe("Kimi");
  expect(menuNames()).toEqual(["Kimi", "Codex"]);
});
it("supports keyboard ordering and changes from another window", async () => {
  await act(async () => root.render(<><AgentManagementControls providerId="kimi" enabled order={["codex","kimi"]} onDragStart={()=>{}} onDragEnd={()=>{}}/><Fixture /></>));
  await act(async () => host.querySelector('[aria-label="调整 Kimi 排序"]')!.dispatchEvent(new KeyboardEvent("keydown", {key:"ArrowUp",bubbles:true})));
  expect(menuNames()).toEqual(["Kimi", "Codex"]);
  await act(async () => { localStorage.setItem(AGENT_PREFERENCES_KEY, JSON.stringify({order:["codex","kimi"],disabled:["kimi"]})); window.dispatchEvent(new StorageEvent("storage",{key:AGENT_PREFERENCES_KEY})); });
  expect(menuNames()).toEqual(["Codex"]);
});
