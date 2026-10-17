// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ProjectId, TaskId, TaskSummary } from "@dogcodes/protocol";
import { ConversationHistory } from "./ConversationHistory";

const project = {additionalDirectories: [],  projectId: "prj_test" as ProjectId, displayName: "dogcodes", path: "/workspace/project", taskCount: 8 };
const tasks: TaskSummary[] = Array.from({ length: 8 }, (_, index) => ({
  taskId: `tsk_${index}` as TaskId, projectId: project.projectId, title: `对话 ${index}`,
  preferredExecutor: { providerId: "codex", installationId: null }, activePrimaryRun: null, streamVersion: 1,
}));
let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
const select = vi.fn();
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  window.localStorage.clear();
  container = document.createElement("div"); document.body.appendChild(container);
  root = createRoot(container); select.mockClear();
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
const render = async (items = tasks, selectedTaskId: TaskId | null = null) => {
  await act(async () => root.render(<ConversationHistory projects={[project]} tasks={items} selectedTaskId={selectedTaskId} onSelect={select} />));
};
it("expands history and keeps selection functional", async () => {
  await render();
  expect(container.querySelectorAll(".task")).toHaveLength(5);
  await act(async () => (container.querySelector(".history-show-more") as HTMLButtonElement).click());
  expect(container.querySelectorAll(".task")).toHaveLength(8);
  await act(async () => (container.querySelectorAll(".task")[7] as HTMLButtonElement).click());
  expect(select).toHaveBeenCalledWith(tasks[0].taskId);
});
it("keeps a selected conversation outside the preview visible", async () => {
  await render(tasks, tasks[0].taskId);
  expect(container.querySelector('[aria-current="page"]')?.textContent).toBe("对话 0");
});
it("shows the last primary Agent independently of the next Agent selection", async () => {
  const task = { ...tasks[0], lastTurnExecutor: { providerId: "cursor", installationId: null } };
  await render([task], task.taskId);
  expect(container.querySelector('.history-agent[aria-label="最后一轮 Agent：Cursor"]')).not.toBeNull();
  await render([{ ...task, preferredExecutor: { providerId: "kimi", installationId: null } }]);
  expect(container.querySelector(".history-agent .agent-cursor")).not.toBeNull();
  const active = { executor: { providerId: "claude-code", installationId: null } } as NonNullable<TaskSummary["activePrimaryRun"]>;
  await render([{ ...task, activePrimaryRun: active }]);
  expect(container.querySelector(".history-agent .agent-claude-code")).not.toBeNull();
  expect(container.querySelector(".task-running-spinner")).not.toBeNull();
  await render([{ ...task, activePrimaryRun: null, lastTurnExecutor: active.executor }]);
  expect(container.querySelector(".history-agent .agent-claude-code")).not.toBeNull();
  expect(container.querySelector(".task-running-spinner")).toBeNull();
});
it("keeps Agent icons in pinned and archived history and falls back for an empty conversation", async () => {
  await render([tasks[0]]);
  expect(container.querySelector('.history-agent[aria-label="会话 Agent：Codex"]')).not.toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="置顶会话"]')!.click());
  expect(container.querySelector(".history-pinned .history-agent .agent-codex")).not.toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="归档会话"]')!.click());
  expect(container.querySelector(".history-archived .history-agent .agent-codex")).not.toBeNull();
});
it("shows ongoing work outside the preview and removes its spinner on completion", async () => {
  const running = tasks.map(task => ({ ...task }));
  // The history only reads the presence of a run, not its execution metadata.
  running[0].activePrimaryRun = { executor: tasks[0].preferredExecutor } as NonNullable<TaskSummary["activePrimaryRun"]>;
  await render(running);
  expect(container.querySelector('[role="status"]')?.closest("button")?.textContent).toBe("对话 0");
  await render(tasks, tasks[0].taskId);
  expect(container.querySelector('[role="status"]')).toBeNull();
  expect(container.querySelector('[aria-current="page"]')).not.toBeNull();
});
it("shows projects that do not have conversations yet", async () => {
  await render([]);
  expect(container.textContent).toContain("dogcodes");
  expect(container.textContent).toContain("暂无对话");
});

it("pins to a separate section, persists on remount, and can unpin without selecting", async () => {
  await render();
  await act(async () => (container.querySelector('[aria-label="置顶会话"]') as HTMLButtonElement).click());
  expect(container.querySelector('[aria-label="已置顶"] .task')?.textContent).toBe("对话 7");
  expect(container.querySelectorAll('.project-group .task-title')[0].textContent).toBe("对话 6");
  expect(select).not.toHaveBeenCalled();
  await act(async () => root.unmount());
  root = createRoot(container);
  await render();
  expect(container.querySelector('[aria-label="已置顶"] .task')?.textContent).toBe("对话 7");
  await act(async () => (container.querySelector('[aria-label="取消置顶"]') as HTMLButtonElement).click());
  expect(container.querySelector('[aria-label="已置顶"]')).toBeNull();
  expect(container.querySelector('.project-group .task-title')?.textContent).toBe("对话 7");
});
it("archives a pinned task without deleting its data and supports restoring", async () => {
  await render();
  await act(async () => (container.querySelector('[aria-label="置顶会话"]') as HTMLButtonElement).click());
  await act(async () => (container.querySelector('.history-pinned [aria-label="归档会话"]') as HTMLButtonElement).click());
  expect(container.querySelector('[aria-label="已置顶"]')).toBeNull();
  expect([...container.querySelectorAll('.task-title')].filter(element => element.closest('.history-row')).map(element => element.textContent)).not.toContain("对话 7");
  expect(tasks[7].title).toBe("对话 7");
  await act(async () => root.unmount()); root = createRoot(container); await render();
  expect(container.querySelector<HTMLElement>('.history-archived .history-section-body')?.hidden).toBe(true);
  expect(container.querySelector('.history-archived-row')?.textContent).toContain("对话 7");
  await act(async () => (container.querySelector('[aria-label="恢复会话：对话 7"]') as HTMLButtonElement).click());
  expect(container.querySelector('.history-archived')).toBeNull();
  expect(container.querySelector('.project-group .task-title')?.textContent).toBe("对话 7");
  expect(select).not.toHaveBeenCalled();
});
it("reports a save failure and leaves the conversation in place", async () => {
  await render();
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage full"); });
  await act(async () => (container.querySelector('[aria-label="归档会话"]') as HTMLButtonElement).click());
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("无法保存");
  expect(container.querySelector('.project-group .task-title')?.textContent).toBe("对话 7");
});


it("places new conversations first without reordering on new messages or mutating input", async () => {
  await render();
  expect([...container.querySelectorAll(".project-group .task-title")].map(node => node.textContent))
    .toEqual(["对话 7", "对话 6", "对话 5", "对话 4", "对话 3"]);
  const updated = tasks.map(task => ({ ...task }));
  updated[0].streamVersion = 100;
  await render(updated);
  expect(container.querySelector(".project-group .task-title")?.textContent).toBe("对话 7");
  await render([...updated, { ...tasks[0], taskId: "tsk_new", title: "最新创建" }]);
  expect(container.querySelector(".project-group .task-title")?.textContent).toBe("最新创建");
  expect(tasks[0].title).toBe("对话 0");
});

it("renames without navigating, trims the name, and keeps input on failure", async () => {
  const rename = vi.fn().mockRejectedValueOnce(new Error("conflict")).mockResolvedValueOnce(undefined);
  await act(async () => root.render(<ConversationHistory projects={[project]} tasks={tasks} selectedTaskId={tasks[0].taskId} onSelect={select} onRename={rename} />));
  await act(async () => (container.querySelector('[aria-label="重命名会话"]') as HTMLButtonElement).click());
  const input = container.querySelector<HTMLInputElement>('[aria-label="会话名称"]')!;
  expect(document.activeElement).toBe(input);
  expect([input.selectionStart, input.selectionEnd]).toEqual([0, input.value.length]);
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "  安全审查  ");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const submit = async () => act(async () => container.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
  await submit();
  expect(rename).toHaveBeenCalledWith(tasks[7], "安全审查");
  expect(input.value).toBe("  安全审查  ");
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("请重试");
  expect(container.querySelector('form [role="alert"]')?.id).toBe(input.getAttribute("aria-describedby"));
  expect(document.activeElement).toBe(input);
  await submit();
  expect(container.querySelector("form")).toBeNull();
  expect(document.activeElement).toBe(document.body);
  expect(select).not.toHaveBeenCalled();
  expect(container.querySelector('[aria-current="page"]')?.textContent).toBe(tasks[0].title);
});

it("cancels a rename with Escape without saving", async () => {
  const rename = vi.fn();
  await act(async () => root.render(<ConversationHistory projects={[project]} tasks={tasks} selectedTaskId={null} onSelect={select} onRename={rename} />));
  await act(async () => (container.querySelector('[aria-label="重命名会话"]') as HTMLButtonElement).click());
  await act(async () => container.querySelector("input")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(container.querySelector("form")).toBeNull();
  expect(rename).not.toHaveBeenCalled();
  expect(document.activeElement?.getAttribute("aria-label")).toBe("重命名会话");
});

it("keeps IME confirmation and cancellation inside the rename input", async () => {
  const rename = vi.fn();
  await act(async () => root.render(<ConversationHistory projects={[project]} tasks={tasks} selectedTaskId={null} onSelect={select} onRename={rename} />));
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="重命名会话"]')!.click());
  const input = container.querySelector("input")!;
  const enter = new KeyboardEvent("keydown", { key: "Enter", isComposing: true, bubbles: true, cancelable: true });
  await act(async () => { input.dispatchEvent(enter); });
  expect(enter.defaultPrevented).toBe(true);
  await act(async () => { input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", isComposing: true, bubbles: true })); });
  expect(container.querySelector("input")).toBe(input);
  expect(rename).not.toHaveBeenCalled();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="取消"]')!.click());
  expect(container.querySelector("form")).toBeNull();
});

it("blocks duplicate submissions and cancellation while a rename is being saved", async () => {
  let finish!: () => void;
  const rename = vi.fn(() => new Promise<void>(resolve => { finish = resolve; }));
  await act(async () => root.render(<ConversationHistory projects={[project]} tasks={tasks} selectedTaskId={null} onSelect={select} onRename={rename} />));
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="重命名会话"]')!.click());
  const input = container.querySelector("input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "新名称");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => {
    const form = container.querySelector("form")!;
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    form.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  });
  expect(rename).toHaveBeenCalledTimes(1);
  expect(input.disabled).toBe(true);
  expect(container.querySelector<HTMLButtonElement>('[aria-label="取消"]')?.disabled).toBe(true);
  expect(container.querySelector('form')?.getAttribute("aria-busy")).toBe("true");
  await act(async () => finish());
  expect(container.querySelector("form")).toBeNull();
  expect(document.activeElement).toBe(document.body);
});

it("closes an unchanged rename without focusing the rename action", async () => {
  const rename = vi.fn();
  await act(async () => root.render(<ConversationHistory projects={[project]} tasks={tasks} selectedTaskId={null} onSelect={select} onRename={rename} />));
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="重命名会话"]')!.click());
  await act(async () => container.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
  expect(container.querySelector("form")).toBeNull();
  expect(rename).not.toHaveBeenCalled();
  expect(document.activeElement).toBe(document.body);
});

it("offers permanent deletion only for archived tasks without a deleted list", async () => {
  const remove = vi.fn();
  const draw = async (items = tasks) => act(async () => root.render(<ConversationHistory projects={[project]} tasks={items} selectedTaskId={null} onSelect={select} onDelete={remove} activeReviewTasks={[tasks[7].taskId]} />));
  await draw();
  expect(container.querySelector('[aria-label="删除会话"]')).toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="置顶会话"]')!.click());
  expect(container.querySelector('.history-pinned [aria-label="删除会话"]')).toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('.history-pinned [aria-label="归档会话"]')!.click());
  expect(container.querySelector<HTMLButtonElement>('.history-archived [aria-label="删除会话"]')!.disabled).toBe(true);
  await draw(tasks.slice(0,7));
  expect(container.querySelector('.history-deleted')).toBeNull();
  expect(container.textContent).not.toContain('最近删除');
  expect(select).not.toHaveBeenCalled();
});


it("collapses each section independently, persists it, and opens project creation from plus", async () => {
  localStorage.setItem("dogcodes.history.preferences.v1", JSON.stringify({[tasks[0].taskId]: "pinned", [tasks[1].taskId]: "archived"}));
  const create = vi.fn();
  const draw = () => root.render(<ConversationHistory projects={[project]} tasks={tasks} selectedTaskId={null} onSelect={select} onCreateProject={create} />);
  await act(async () => draw());
  for (const section of ["pinned", "projects", "archived"]) {
    const toggle = container.querySelector<HTMLButtonElement>(`.history-${section} .history-section-toggle`)!;
    const before = toggle.getAttribute("aria-expanded");
    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-expanded")).toBe(before === "true" ? "false" : "true");
  }
  expect(container.querySelector<HTMLElement>(".history-pinned .history-section-body")?.hidden).toBe(true);
  expect(container.querySelector<HTMLElement>(".history-projects .history-section-body")?.hidden).toBe(true);
  expect(container.querySelector<HTMLElement>(".history-archived .history-section-body")?.hidden).toBe(false);
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="新建项目"]')!.click());
  expect(create).toHaveBeenCalledOnce();
  await act(async () => root.unmount()); root = createRoot(container);
  await act(async () => draw());
  expect(container.querySelector<HTMLElement>(".history-projects .history-section-body")?.hidden).toBe(true);
  expect(container.querySelector<HTMLElement>(".history-archived .history-section-body")?.hidden).toBe(false);
});

it("sorts projects and their conversations by latest activity and restores manual order", async () => {
  const other = {...project, projectId: "prj_other" as ProjectId, displayName: "Another", updatedAt: "2026-09-10T00:00:00Z"};
  let items = tasks.slice(0, 3).map((task, index) => ({...task, updatedAt: `2026-09-0${3-index}T00:00:00Z`}));
  const draw = () => root.render(<ConversationHistory projects={[project, other]} tasks={items} selectedTaskId={null} onSelect={select} />);
  const choose = async (label: string) => {
    await act(async () => container.querySelector<HTMLButtonElement>('button[aria-label="项目排序方式"]')!.click());
    await act(async () => [...container.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')].find(button => button.textContent?.includes(label))!.click());
  };
  await act(async () => draw());
  await choose("最近更新");
  expect(container.querySelector(".project-group")?.getAttribute("aria-label")).toBe("Another");
  expect([...container.querySelectorAll(".project-group .task-title")].map(item => item.textContent)).toEqual(["对话 0", "对话 1", "对话 2"]);
  items = items.map((item, index) => index === 2 ? {...item, updatedAt: "2026-09-20T00:00:00Z"} : item);
  await act(async () => draw());
  expect(container.querySelector(".project-group")?.getAttribute("aria-label")).toBe("dogcodes");
  expect(container.querySelector(".project-group .task-title")?.textContent).toBe("对话 2");
  await choose("手动排序");
  expect([...container.querySelectorAll(".project-group .task-title")].map(item => item.textContent)).toEqual(["对话 2", "对话 1", "对话 0"]);
});

it.each(["pinned", "archived"])("persists newest-first sorting for %s without changing the other sections", async section => {
  const title = section === "pinned" ? "已置顶" : "已归档";
  const items = tasks.slice(0, 3).map((task, index) => ({...task, updatedAt: `2026-09-0${3-index}T00:00:00Z`}));
  localStorage.setItem("dogcodes.history.preferences.v1", JSON.stringify(Object.fromEntries(items.map(task => [task.taskId, section]))));
  const draw = () => root.render(<ConversationHistory projects={[project]} tasks={items} selectedTaskId={null} onSelect={select} />);
  await act(async () => draw());
  await act(async () => container.querySelector<HTMLButtonElement>(`button[aria-label="${title}排序方式"]`)!.click());
  await act(async () => [...container.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')].find(button => button.textContent?.includes("最近更新"))!.click());
  expect([...container.querySelectorAll(`.history-${section} .task-title`)].map(item => item.textContent)).toEqual(["对话 0", "对话 1", "对话 2"]);
  await act(async () => root.unmount()); root = createRoot(container); await act(async () => draw());
  expect([...container.querySelectorAll(`.history-${section} .task-title`)].map(item => item.textContent)).toEqual(["对话 0", "对话 1", "对话 2"]);
  await act(async () => container.querySelector<HTMLButtonElement>('button[aria-label="项目排序方式"]')!.click());
  expect(container.querySelector('[role="menuitemradio"][aria-checked="true"]')?.textContent).toBe("手动排序✓");
});

it("reorders manual conversations and projects with the keyboard and retains order on remount", async () => {
  const other = {...project, projectId: "prj_other" as ProjectId, displayName: "Another"};
  const draw = () => root.render(<ConversationHistory projects={[project, other]} tasks={tasks.slice(0,3)} selectedTaskId={null} onSelect={select} />);
  await act(async () => draw());
  await act(async () => container.querySelector('.task')!.dispatchEvent(new KeyboardEvent("keydown", {key:"ArrowDown", altKey:true, bubbles:true})));
  expect([...container.querySelectorAll('.task-title')].map(item => item.textContent)).toEqual(["对话 1", "对话 2", "对话 0"]);
  await act(async () => container.querySelector('.project-heading-trigger')!.dispatchEvent(new KeyboardEvent("keydown", {key:"ArrowDown", altKey:true, bubbles:true})));
  expect(container.querySelector('.project-group')?.getAttribute('aria-label')).toBe("Another");
  await act(async () => root.unmount()); root = createRoot(container); await act(async () => draw());
  expect(container.querySelector('.project-group')?.getAttribute('aria-label')).toBe("Another");
  expect([...container.querySelectorAll('.task-title')].map(item => item.textContent)).toEqual(["对话 1", "对话 2", "对话 0"]);
});

it.each(["projects", "pinned", "archived"])("applies and persists %s sorting after a WKWebView pointer blur", async section => {
  const label = section === "projects" ? "项目" : section === "pinned" ? "已置顶" : "已归档";
  const items = tasks.slice(0, 3).map((task, index) => ({ ...task, updatedAt: `2026-09-0${3-index}T00:00:00Z` }));
  if (section !== "projects") localStorage.setItem("dogcodes.history.preferences.v1", JSON.stringify(Object.fromEntries(items.map(task => [task.taskId, section]))));
  await render(items);
  const openMenu = async () => act(async () => container.querySelector<HTMLButtonElement>(`button[aria-label="${label}排序方式"]`)!.click());
  await openMenu();
  const recent = container.querySelector<HTMLButtonElement>('[role="menuitemradio"][aria-checked="false"]')!;
  // WKWebView blurs the focused item on mousedown without focusing the clicked
  // button. React processes this before mouseup/click, so it must not unmount it.
  await act(async () => recent.dispatchEvent(new Event("pointerdown", { bubbles: true })));
  await act(async () => {
    recent.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    (document.activeElement as HTMLElement).blur();
  });
  expect(recent.isConnected).toBe(true);
  await act(async () => {
    recent.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    recent.click();
  });
  expect([...container.querySelectorAll(`.history-${section} .task-title`)].map(item => item.textContent)).toEqual(["对话 0", "对话 1", "对话 2"]);
  await act(async () => root.unmount()); root = createRoot(container); await render(items);
  await openMenu();
  expect(container.querySelector('[role="menuitemradio"][aria-checked="true"]')?.textContent).toBe("最近更新✓");
});

it("opens sorting from an icon and closes the menu on outside pointer, keyboard focus and Escape", async () => {
  await render();
  const trigger = container.querySelector<HTMLButtonElement>('button[aria-label="项目排序方式"]')!;
  expect(trigger.querySelector("svg")).not.toBeNull();
  expect(trigger.textContent).not.toContain("⋯");
  expect(container.querySelector('[role="menu"]')).toBeNull();
  await act(async () => trigger.click());
  expect(container.querySelector('[role="menu"]')).not.toBeNull();
  await act(async () => document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })));
  expect(container.querySelector('[role="menu"]')).toBeNull();
  await act(async () => trigger.click());
  await act(async () => container.querySelector<HTMLButtonElement>('.task')!.focus());
  expect(container.querySelector('[role="menu"]')).toBeNull();
  await act(async () => trigger.click());
  await act(async () => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(container.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
it("persists project pinning and removal while preserving conversations for restoration", async () => {
  const second = { ...project, projectId: "prj_second" as ProjectId, displayName: "Another" };
  const show = async () => act(async () => root.render(<ConversationHistory projects={[project, second]} tasks={tasks} selectedTaskId={null} onSelect={select} />));
  const menuAction = async (name: string, action: string) => {
    await act(async () => container.querySelector<HTMLButtonElement>(`[aria-label="${name} 项目操作"]`)!.click());
    await act(async () => [...container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(button => button.textContent === action)!.click());
  };
  await show();
  await menuAction("Another", "置顶项目");
  expect(container.querySelector('.project-group')?.getAttribute('aria-label')).toBe("Another");
  await act(async () => root.unmount()); root = createRoot(container); await show();
  expect(container.querySelector('.project-group')?.getAttribute('aria-label')).toBe("Another");
  await menuAction("dogcodes", "移除项目");
  expect(container.querySelector('.project-group[aria-label="dogcodes"]')).toBeNull();
  await act(async () => root.unmount()); root = createRoot(container); await show();
  expect(container.querySelector('.project-group[aria-label="dogcodes"]')).toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('.removed-projects button')!.click());
  expect(container.querySelectorAll('.project-group[aria-label="dogcodes"] .task')).toHaveLength(5);
  expect(tasks).toHaveLength(8);
  expect(select).not.toHaveBeenCalled();
});

const dragEvent = (target: Element, type: string, clientY = 110, relatedTarget: EventTarget | null = null) => {
  const event = new MouseEvent(type, { bubbles: true, cancelable: true, clientY, relatedTarget });
  Object.defineProperty(event, "dataTransfer", { value: { effectAllowed: "none", dropEffect: "none", setData: vi.fn() } });
  target.dispatchEvent(event);
  return event;
};
const mockDropBounds = (element: Element) => vi.spyOn(element, "getBoundingClientRect").mockReturnValue({ top: 100, height: 40, bottom: 140, left: 0, right: 240, width: 240, x: 0, y: 100, toJSON: () => ({}) });

it.each(["projects", "pinned", "archived"])("previews and persists both insertion edges in %s in either direction", async section => {
  if (section !== "projects") localStorage.setItem("dogcodes.history.preferences.v1", JSON.stringify(Object.fromEntries(tasks.slice(0, 3).map(task => [task.taskId, section]))));
  for (const [sourceIndex, targetIndex, edge, expected] of [
    [0, 2, "before", [1, 2, 0]], [0, 2, "after", [1, 0, 2]],
    [2, 0, "before", [0, 2, 1]], [2, 0, "after", [2, 0, 1]],
  ] as const) {
    localStorage.removeItem("dogcodes.history.sections.v1");
    await render(tasks.slice(0, 3));
    if (section === "archived") await act(async () => container.querySelector<HTMLButtonElement>(".history-archived .history-section-toggle")!.click());
    const rows = container.querySelectorAll(`.history-${section} ${section === "archived" ? ".history-archived-row" : ".history-row"}`);
    const source = rows[sourceIndex], target = rows[targetIndex];
    mockDropBounds(target);
    await act(async () => { dragEvent(source, "dragstart"); });
    await act(async () => { dragEvent(target.querySelector(".task-title")!, "dragover", edge === "before" ? 110 : 130); });
    expect(target.getAttribute("data-drop-edge")).toBe(edge);
    expect(container.querySelectorAll("[data-drop-edge]")).toHaveLength(1);
    await act(async () => { dragEvent(target, "drop", edge === "before" ? 110 : 130); });
    expect(container.querySelector("[data-drop-edge]")).toBeNull();
    const titles = () => [...container.querySelectorAll(`.history-${section} .task-title`)].map(item => item.textContent);
    expect(titles()).toEqual(expected.map(index => `对话 ${index}`));
    await act(async () => root.unmount()); root = createRoot(container);
    await render(tasks.slice(0, 3));
    expect(titles()).toEqual(expected.map(index => `对话 ${index}`));
    await act(async () => root.unmount()); root = createRoot(container);
  }
});

it("uses the entire project boundary when dragging over its header or conversations", async () => {
  const other = { ...project, projectId: "prj_other" as ProjectId, displayName: "Another" };
  await act(async () => root.render(<ConversationHistory projects={[project, other]} tasks={tasks.slice(0, 3)} selectedTaskId={null} onSelect={select} />));
  const [target, source] = container.querySelectorAll(".project-group");
  mockDropBounds(target);
  await act(async () => { dragEvent(source.querySelector(".project-heading-trigger")!, "dragstart"); });
  await act(async () => { dragEvent(target.querySelector(".project-heading-trigger")!, "dragover", 110); });
  expect(target.getAttribute("data-drop-edge")).toBe("before");
  await act(async () => { dragEvent(target.querySelector(".task-title")!, "dragover", 130); });
  expect(target.getAttribute("data-drop-edge")).toBe("after");
  expect(container.querySelectorAll("[data-drop-edge]")).toHaveLength(1);
  await act(async () => { dragEvent(target.querySelector(".task-title")!, "dragover", 110); });
  await act(async () => { dragEvent(target.querySelector(".task-title")!, "drop", 110); });
  expect(container.querySelector(".project-group")?.getAttribute("aria-label")).toBe("Another");
  expect(container.querySelector("[data-drop-edge]")).toBeNull();
});

it("clears the insertion line over invalid targets, outside the list, and on cancellation", async () => {
  await render(tasks.slice(0, 3));
  const [source, target] = container.querySelectorAll(".history-row");
  const list = container.querySelector(".conversation-list")!;
  mockDropBounds(target);
  await act(async () => { dragEvent(source, "dragstart"); });
  for (const clear of [
    () => dragEvent(source, "dragover"),
    () => dragEvent(container.querySelector(".project-heading-trigger")!, "dragover"),
    () => dragEvent(list, "dragover"),
    () => dragEvent(target, "dragleave", 110, document.body),
    () => dragEvent(source, "dragend"),
  ]) {
    await act(async () => { dragEvent(target, "dragover"); });
    expect(target.getAttribute("data-drop-edge")).toBe("before");
    // Entering a child inside the same row must preserve the line.
    await act(async () => { dragEvent(target, "dragleave", 110, target.querySelector(".task-title")); });
    expect(target.getAttribute("data-drop-edge")).toBe("before");
    await act(async () => { clear(); });
    expect(container.querySelector("[data-drop-edge]")).toBeNull();
  }
  // An external drag has no internal source, so it cannot display a reorder hint.
  await act(async () => { dragEvent(target, "dragover"); });
  expect(container.querySelector("[data-drop-edge]")).toBeNull();
  expect([...container.querySelectorAll(".task-title")].map(item => item.textContent)).toEqual(["对话 2", "对话 1", "对话 0"]);
});

it("does not advertise project drops across the pinned boundary", async () => {
  const other = { ...project, projectId: "prj_other" as ProjectId, displayName: "Another" };
  await act(async () => root.render(<ConversationHistory projects={[project, other]} tasks={[]} selectedTaskId={null} onSelect={select} />));
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="dogcodes 项目操作"]')!.click());
  await act(async () => [...container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(button => button.textContent === "置顶项目")!.click());
  const [target, source] = container.querySelectorAll(".project-group");
  await act(async () => { dragEvent(source.querySelector(".project-heading-trigger")!, "dragstart"); });
  let over!: MouseEvent;
  await act(async () => { over = dragEvent(target, "dragover"); });
  expect(over.defaultPrevented).toBe(false);
  expect(container.querySelector("[data-drop-edge]")).toBeNull();
  await act(async () => { dragEvent(target, "drop"); });
  expect(container.querySelector(".project-group")?.getAttribute("aria-label")).toBe("dogcodes");
});
