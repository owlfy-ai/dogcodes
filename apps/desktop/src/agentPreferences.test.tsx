// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { AGENT_PREFERENCES_KEY, agentDropSlot, enabledAgents, moveAgent, moveAgentToSlot, orderAgents, readAgentPreferences, setAgentEnabled, useAgentPreferences } from "./agentPreferences";
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
function dragHarness() {
  const data = new Map<string,string>();
  const transfer = { types: [AGENT_DRAG_TYPE], setData: (key:string,value:string) => data.set(key,value), getData: (key:string) => data.get(key), effectAllowed: "", dropEffect: "" };
  const dispatch = async (node: Element, type: string, point: { clientX?: number; clientY?: number } = {}) => act(async () => {
    const event = new Event(type, { bubbles: true, cancelable: true });
    Object.defineProperties(event, { dataTransfer: { value: transfer }, clientX: { value: point.clientX }, clientY: { value: point.clientY } });
    node.dispatchEvent(event);
  });
  return { transfer, dispatch };
}
const card = (name: string) => host.querySelector<HTMLElement>(`.agent-management-card[aria-label="${name}"]`)!;
const handle = (name: string) => host.querySelector(`[aria-label="调整 ${name} 排序"]`)!;
const edges = () => [...host.querySelectorAll('.agent-management-card')].map(node => node.getAttribute('data-drop-edge'));
/** Lays the cards out at fixed boxes: one column stacks them, two columns fill rows left to right. */
function layOut(columns: 1 | 2) {
  const grid = host.querySelector<HTMLElement>('.agent-management-grid')!;
  grid.style.gridTemplateColumns = columns === 2 ? "500px 500px" : "500px";
  grid.getBoundingClientRect = () => ({ left: 0, top: 0, right: 1016, bottom: 1000, width: 1016, height: 1000 }) as DOMRect;
  host.querySelectorAll<HTMLElement>('.agent-management-card').forEach((node, index) => {
    const left = columns === 2 ? (index % 2) * 516 : 0, top = (columns === 2 ? Math.floor(index / 2) : index) * 216;
    node.getBoundingClientRect = () => ({ left, top, right: left + 500, bottom: top + 200, width: 500, height: 200 }) as DOMRect;
  });
}
it("computes insertion slots and ignores drops that keep the order", () => {
  const order = ["codex", "cursor", "kimi"];
  expect(agentDropSlot("kimi", "codex", "before", order)).toBe(0);
  expect(agentDropSlot("kimi", "cursor", "after", order)).toBeNull();
  expect(agentDropSlot("kimi", "kimi", "before", order)).toBeNull();
  expect(agentDropSlot("codex", "kimi", "after", order)).toBe(3);
  expect(agentDropSlot("codex", "cursor", "before", order)).toBeNull();
  moveAgentToSlot("codex", 3, order);
  expect(readAgentPreferences().order).toEqual(["cursor", "kimi", "codex"]);
  moveAgentToSlot("codex", 2, ["cursor", "kimi", "codex"]);
  expect(readAgentPreferences().order).toEqual(["cursor", "kimi", "codex"]);
});
it("shows the drop slot while dragging and drops exactly there, then applies it to the shared picker", async () => {
  await act(async () => root.render(<Fixture />));
  layOut(1);
  const { dispatch } = dragHarness();
  await dispatch(handle("Kimi"), "dragstart");
  await act(async () => { await new Promise(resolve => requestAnimationFrame(resolve)); });
  expect(card("Kimi").hasAttribute("data-dragging")).toBe(true);
  // Lower half of Codex: between Codex and Cursor.
  await dispatch(card("Codex"), "dragover", { clientX: 100, clientY: 150 });
  expect(card("Codex").getAttribute("data-drop-edge")).toBe("after");
  expect(host.querySelector('.agent-management-grid')!.getAttribute("data-dragging")).toBe("y");
  // Upper half of Codex: first place.
  await dispatch(card("Codex"), "dragover", { clientX: 100, clientY: 20 });
  expect(edges()).toEqual(["before", null, null]);
  // The gap between cards keeps the last indicator.
  await dispatch(host.querySelector('.agent-management-grid')!, "dragover", { clientX: 100, clientY: 208 });
  expect(edges()).toEqual(["before", null, null]);
  await dispatch(card("Codex"), "drop", { clientX: 100, clientY: 20 });
  expect(cardNames()).toEqual(["Kimi", "Codex", "Cursor"]);
  expect(edges()).toEqual([null, null, null]);
  expect(host.querySelector('[role="status"]')!.textContent).toBe("Kimi 已移到第 1 位，共 3 个");
  expect(menuNames()).toEqual(["Kimi", "Codex"]);
  await act(async () => root.render(<Fixture key="remount" />));
  expect(cardNames()[0]).toBe("Kimi");
  expect(menuNames()).toEqual(["Kimi", "Codex"]);
});
it("uses left and right halves in the two-column grid", async () => {
  await act(async () => root.render(<Fixture />));
  layOut(2); // Codex | Cursor / Kimi
  const { dispatch } = dragHarness();
  await dispatch(handle("Codex"), "dragstart");
  // Vertically high on Kimi but in its right half: after Kimi, i.e. last.
  await dispatch(card("Kimi"), "dragover", { clientX: 400, clientY: 230 });
  expect(card("Kimi").getAttribute("data-drop-edge")).toBe("after");
  expect(host.querySelector('.agent-management-grid')!.getAttribute("data-dragging")).toBe("x");
  // Left half of Cursor is right after Codex itself: no indicator, no change.
  await dispatch(card("Cursor"), "dragover", { clientX: 560, clientY: 100 });
  expect(edges()).toEqual([null, null, null]);
  await dispatch(card("Cursor"), "dragover", { clientX: 900, clientY: 100 });
  expect(card("Cursor").getAttribute("data-drop-edge")).toBe("after");
  await dispatch(card("Cursor"), "drop", { clientX: 900, clientY: 100 });
  expect(cardNames()).toEqual(["Cursor", "Codex", "Kimi"]);
});
it("keeps the order when dropped in place or after leaving the cards", async () => {
  await act(async () => root.render(<Fixture />));
  layOut(1);
  const { dispatch } = dragHarness();
  await dispatch(handle("Cursor"), "dragstart");
  await dispatch(card("Cursor"), "dragover", { clientX: 100, clientY: 230 });
  await dispatch(card("Codex"), "dragover", { clientX: 100, clientY: 150 });
  expect(edges()).toEqual([null, null, null]);
  await dispatch(card("Cursor"), "drop", { clientX: 100, clientY: 230 });
  expect(cardNames()).toEqual(["Codex", "Cursor", "Kimi"]);
  expect(localStorage.getItem(AGENT_PREFERENCES_KEY)).toBeNull();
  await dispatch(handle("Cursor"), "dragstart");
  await dispatch(card("Codex"), "dragover", { clientX: 100, clientY: 20 });
  expect(edges()).toEqual(["before", null, null]);
  await dispatch(host.querySelector('.agent-management-grid')!, "dragleave", { clientX: 2000, clientY: 20 });
  expect(edges()).toEqual([null, null, null]);
  await dispatch(handle("Cursor"), "dragend");
  expect(cardNames()).toEqual(["Codex", "Cursor", "Kimi"]);
  expect(card("Cursor").hasAttribute("data-dragging")).toBe(false);
});
it("supports keyboard ordering and changes from another window", async () => {
  await act(async () => root.render(<><AgentManagementControls providerId="kimi" enabled order={["codex","kimi"]} onDragStart={()=>{}} onDragEnd={()=>{}}/><Fixture /></>));
  await act(async () => host.querySelector('[aria-label="调整 Kimi 排序"]')!.dispatchEvent(new KeyboardEvent("keydown", {key:"ArrowUp",bubbles:true})));
  expect(menuNames()).toEqual(["Kimi", "Codex"]);
  // The management page's own handles announce the new position.
  const kimi = host.querySelectorAll('[aria-label="调整 Kimi 排序"]')[1]!;
  expect(kimi.getAttribute("aria-keyshortcuts")).toContain("ArrowDown");
  await act(async () => kimi.dispatchEvent(new KeyboardEvent("keydown", {key:"ArrowDown",bubbles:true})));
  expect(cardNames()).toEqual(["Codex", "Kimi", "Cursor"]);
  expect(host.querySelector('[role="status"]')!.textContent).toBe("Kimi 已移到第 2 位，共 3 个");
  await act(async () => { localStorage.setItem(AGENT_PREFERENCES_KEY, JSON.stringify({order:["codex","kimi"],disabled:["kimi"]})); window.dispatchEvent(new StorageEvent("storage",{key:AGENT_PREFERENCES_KEY})); });
  expect(menuNames()).toEqual(["Codex"]);
});
