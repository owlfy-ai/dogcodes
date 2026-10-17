// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import type { EventStreamMessage, HistoryPreferences, HistoryPreferencesSnapshot, ProjectSummary, TaskSummary } from "@dogcodes/protocol";
import { SharedHistoryPreferences, emptyHistoryPreferences } from "./sharedHistoryPreferences";
import { SharedHistoryPreferencesGate, readLegacyHistoryPreferences } from "./SharedHistoryPreferencesGate";
import { ConversationHistory } from "./ConversationHistory";
import { COLLAPSED_STORAGE_KEY, SECTION_STORAGE_KEY } from "./historySections";

const api = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn(), subscribe: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/path", () => ({ homeDir: async () => "/home/test" }));
vi.mock("./tauriClient", () => ({ TauriCoreClient: class { historyPreferences = api.read; setHistoryPreferences = api.write; subscribe = api.subscribe; } }));

const project: ProjectSummary = { projectId: "prj_test", path: "/workspace", additionalDirectories: [], displayName: "Workspace", taskCount: 2 };
const tasks: TaskSummary[] = [0, 1].map<TaskSummary>(index => ({ taskId: `tsk_${index}`, projectId: project.projectId, title: `Task ${index}`, preferredExecutor: { providerId: "codex", installationId: null }, activePrimaryRun: null, streamVersion: 1 }));
let root: Root, element: HTMLDivElement;
let current: HistoryPreferencesSnapshot;
const listeners = new Set<(message: EventStreamMessage) => void>();
const stores: SharedHistoryPreferences[] = [];
const legacyKey = "dogcodes.history.preferences.v1";
const projectKey = "dogcodes.history.projects.v1";
function commit(preferences: HistoryPreferences) {
  current = { revision: current.revision + 1, lastSequence: current.lastSequence + 1, preferences: structuredClone(preferences) };
  const message: EventStreamMessage = { type: "event", data: {
    schemaVersion: 1, eventId: "event", sequence: current.lastSequence, streamVersion: current.revision,
    stream: { streamType: "system", streamId: "preferences.history" }, eventType: "preferences.history.updated",
    payload: structuredClone(preferences), source: { kind: "hub" }, occurredAt: "now", recordedAt: "now",
    correlationId: "correlation", causationId: null, vendorExtensions: null,
  } };
  for (const listener of listeners) listener(message);
}
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); localStorage.clear(); listeners.clear();
  current = { revision: 0, lastSequence: 30, preferences: emptyHistoryPreferences() };
  api.read.mockReset().mockImplementation(async () => structuredClone(current));
  api.write.mockReset().mockImplementation(async (value: HistoryPreferences, revision: number) => {
    if (current.revision !== revision) throw new Error("conflict");
    commit(value);
  });
  api.subscribe.mockReset().mockImplementation(async (_after: number, listener: (message: EventStreamMessage) => void) => {
    listeners.add(listener); return () => listeners.delete(listener);
  });
  element = document.createElement("div"); document.body.append(element); root = createRoot(element);
});
afterEach(async () => { await act(async () => root.unmount()); for (const store of stores.splice(0)) store.stop(); element.remove(); localStorage.clear(); vi.unstubAllGlobals(); });
const render = () => act(async () => root.render(<SharedHistoryPreferencesGate><ConversationHistory projects={[project]} tasks={tasks} selectedTaskId={null} onSelect={() => {}} /></SharedHistoryPreferencesGate>));
const button = (label: string) => [...element.querySelectorAll<HTMLButtonElement>("button")].find(node => node.textContent === label || node.getAttribute("aria-label") === label)!;
async function open(legacy = emptyHistoryPreferences()) {
  const store = new SharedHistoryPreferences({ historyPreferences: api.read, setHistoryPreferences: api.write, subscribe: api.subscribe }, legacy);
  stores.push(store); store.start(); for (let i = 0; i < 12; i++) await Promise.resolve(); return store;
}

it("imports business organization explicitly, leaves collapsed layout local and preserves every old copy", async () => {
  const legacy = JSON.stringify({ tsk_0: "archived", tsk_1: "pinned" });
  const oldSections = JSON.stringify({ sections: { projects: { collapsed: true, sort: "updated" } }, orders: { projects: [project.projectId] } });
  localStorage.setItem(legacyKey, legacy); localStorage.setItem(SECTION_STORAGE_KEY, oldSections);
  localStorage.setItem(projectKey, JSON.stringify({ [project.projectId]: "pinned" }));
  const expected = readLegacyHistoryPreferences();
  await render();
  expect(api.write).not.toHaveBeenCalled();
  expect(element.textContent).toContain("保留已有历史列表设置");
  await act(async () => button("保留并同步现有设置").click());
  expect(current.preferences).toEqual(expected);
  expect(JSON.stringify(current.preferences)).not.toContain("collapsed");
  expect(element.querySelector(".history-archived-row")?.textContent).toContain("Task 0");
  expect(element.querySelector(".history-pinned .task-title")?.textContent).toBe("Task 1");
  expect(element.querySelector<HTMLElement>(".history-projects .history-section-body")?.hidden).toBe(true);
  await act(async () => element.querySelector<HTMLButtonElement>(".history-projects .history-section-toggle")!.click());
  expect(api.write).toHaveBeenCalledTimes(1);
  expect(localStorage.getItem(legacyKey)).toBe(legacy);
  expect(localStorage.getItem(SECTION_STORAGE_KEY)).toBe(oldSections);
  expect(JSON.parse(localStorage.getItem(COLLAPSED_STORAGE_KEY)!).projects).toBe(false);
});

it("does not let a stale local backup override host state and reflects another client's archive", async () => {
  current.revision = 5;
  localStorage.setItem(legacyKey, JSON.stringify({ tsk_0: "archived" }));
  await render();
  expect(element.textContent).not.toContain("保留已有");
  expect(element.querySelector(".history-archived")).toBeNull();
  const other = await open();
  await act(async () => other.write({ ...current.preferences, tasks: { tsk_0: "archived" } }));
  expect(element.querySelector(".history-archived-row")?.textContent).toContain("Task 0");
  await act(async () => button("恢复会话：Task 0").click());
  expect(other.getSnapshot().snapshot?.preferences.tasks).toEqual({});
  expect(localStorage.getItem(legacyKey)).toBe(JSON.stringify({ tsk_0: "archived" }));
});

it("keeps an unconfirmed archive in place, disables more mutations and retries the same operation", async () => {
  current.revision = 1;
  await render();
  api.write.mockRejectedValueOnce(new Error("reply lost"));
  await act(async () => button("归档会话").click());
  const request = api.write.mock.calls[0];
  expect(element.querySelector(".history-archived")).toBeNull();
  expect(button("归档会话").disabled).toBe(true);
  expect(element.textContent).toContain("保存结果尚未确认");
  // Layout is independent even while a business operation is unresolved.
  await act(async () => element.querySelector<HTMLButtonElement>(".history-projects .history-section-toggle")!.click());
  expect(api.write).toHaveBeenCalledTimes(1);
  await act(async () => button("重新核实列表设置").click());
  expect(api.write.mock.calls[1]).toEqual(request);
  expect(element.querySelector(".history-archived-row")).not.toBeNull();
});

it("does not overwrite another client's winning initialization or later conflicting edit", async () => {
  const a = await open({ ...emptyHistoryPreferences(), tasks: { tsk_0: "pinned" } });
  const b = await open({ ...emptyHistoryPreferences(), projects: { prj_test: "removed" } });
  await a.importLegacy(true); await b.importLegacy(true);
  expect(current.preferences.tasks).toEqual({ tsk_0: "pinned" });
  expect(api.write).toHaveBeenCalledTimes(1);
  // Drop one client's notifications to force a CAS conflict, not a happy-path merge.
  listeners.clear();
  await a.write({ ...current.preferences, tasks: { tsk_1: "archived" } });
  await b.write({ ...b.getSnapshot().snapshot!.preferences, projects: { prj_test: "removed" } });
  expect(current.preferences.tasks).toEqual({ tsk_1: "archived" });
  expect(current.preferences.projects).toEqual({});
  expect(b.getSnapshot().snapshot?.preferences).toEqual(current.preferences);
  expect(b.getSnapshot().error).toContain("后台当前设置");
});

it("saves sorting and manual orders to the host without changing another section or local backup", async () => {
  current.revision = 1;
  current.preferences.tasks = { tsk_0: "pinned" };
  const backup = JSON.stringify({ orders: { projects: ["older"] } });
  localStorage.setItem(SECTION_STORAGE_KEY, backup);
  await render();
  await act(async () => button("项目排序方式").click());
  await act(async () => [...element.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')].find(node => node.textContent?.includes("最近更新"))!.click());
  expect(current.preferences.sorts).toEqual({ projects: "updated" });
  expect(current.preferences.tasks).toEqual({ tsk_0: "pinned" });
  expect(localStorage.getItem(SECTION_STORAGE_KEY)).toBe(backup);
  // Switch back, restore the pinned task and reorder actual rows through keyboard interaction.
  await act(async () => button("项目排序方式").click());
  await act(async () => [...element.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')].find(node => node.textContent?.includes("手动排序"))!.click());
  await act(async () => button("取消置顶").click());
  const row = element.querySelector<HTMLElement>(".project-group .history-row")!;
  await act(async () => row.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", altKey: true, bubbles: true })));
  expect(current.preferences.orders["project:prj_test"]).toEqual(["tsk_0", "tsk_1"]);
  expect(localStorage.getItem(SECTION_STORAGE_KEY)).toBe(backup);
});

it("offers defaults for invalid legacy entries without deleting the backup", async () => {
  const backup = JSON.stringify({ "bad\nkey": "archived" });
  localStorage.setItem(legacyKey, backup);
  await render();
  await act(async () => button("保留并同步现有设置").click());
  expect(api.write).not.toHaveBeenCalled();
  expect(element.textContent).toContain("包含无效内容");
  await act(async () => button("使用默认设置").click());
  expect(current.preferences).toEqual(emptyHistoryPreferences());
  expect(localStorage.getItem(legacyKey)).toBe(backup);
});

it("keeps layout-only legacy settings local without asking for a business-state import", async () => {
  localStorage.setItem(SECTION_STORAGE_KEY, JSON.stringify({ sections: { projects: { collapsed: true, sort: "manual" } } }));
  await render();
  expect(element.textContent).not.toContain("保留已有历史列表设置");
  expect(element.querySelector<HTMLElement>(".history-projects .history-section-body")?.hidden).toBe(true);
  expect(api.write).not.toHaveBeenCalled();
});

it("removes and restores only the project's list membership and keeps its conversations", async () => {
  current.revision = 1;
  await render();
  await act(async () => button("Workspace 项目操作").click());
  await act(async () => [...element.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(node => node.textContent?.includes("移除项目"))!.click());
  expect(current.preferences.projects).toEqual({ prj_test: "removed" });
  expect(element.querySelector(".project-group")).toBeNull();
  expect(tasks).toHaveLength(2);
  await act(async () => button("恢复项目：Workspace").click());
  expect(current.preferences.projects).toEqual({});
  expect(element.querySelectorAll(".project-group .task")).toHaveLength(2);
});

it("offers pre-rename backups for import only when there is no current-name value", async () => {
  const oldKey = "codesheepdog.history.preferences.v1";
  const backup = JSON.stringify({ tsk_0: "archived" });
  localStorage.setItem(oldKey, backup);
  localStorage.setItem("codesheepdog.history.projects.v1", JSON.stringify({ prj_test: "pinned" }));
  localStorage.setItem("codesheepdog.history.sections.v1", JSON.stringify({ sections: { projects: { sort: "updated", collapsed: true } } }));
  await render();
  expect(element.textContent).toContain("保留已有历史列表设置");
  await act(async () => button("保留并同步现有设置").click());
  expect(current.preferences.tasks).toEqual({ tsk_0: "archived" });
  expect(current.preferences.projects).toEqual({ prj_test: "pinned" });
  expect(current.preferences.sorts.projects).toBe("updated");
  expect(localStorage.getItem(oldKey)).toBe(backup);
  localStorage.setItem(legacyKey, "{}");
  expect(readLegacyHistoryPreferences().tasks).toEqual({});
});
