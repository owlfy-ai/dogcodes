// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ProjectId } from "@dogcodes/protocol";
import { ProjectHoverCard } from "./ProjectHoverCard";

let root: Root, container: HTMLDivElement;
const project = { projectId: "prj_test" as ProjectId, displayName: "DrillHUD", path: "/workspace/DrillHUD", additionalDirectories: ["/workspace/shared", "/workspace/server"], taskCount: 8 };
const edit = vi.fn();
beforeEach(async () => {
  vi.useFakeTimers(); vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); edit.mockClear();
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
  await act(async () => root.render(<ProjectHoverCard project={project} onEdit={edit} />));
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });
it("shows every directory and the full conversation count, and stays open while crossing into the card", async () => {
  const header = container.querySelector("header")!;
  await act(async () => { header.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })); vi.advanceTimersByTime(280); });
  expect(container.querySelectorAll(".project-hover-directories li")).toHaveLength(3);
  expect(container.querySelector(".project-hover-count")?.textContent).toBe("8 个会话");
  expect(container.querySelector(".project-hover-directories")?.textContent).toContain("/workspace/shared");
  await act(async () => { header.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body })); vi.advanceTimersByTime(100); });
  await act(async () => { container.querySelector(".project-hover-card")!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body })); vi.advanceTimersByTime(300); });
  expect(container.querySelector(".project-hover-card")).not.toBeNull();
  await act(async () => { container.querySelector<HTMLButtonElement>(".project-hover-edit")!.click(); });
  expect(edit).toHaveBeenCalledWith(project);
  expect(container.querySelector(".project-hover-card")).toBeNull();
});
it("supports keyboard focus and Escape and dismisses after the pointer leaves", async () => {
  const trigger = container.querySelector<HTMLButtonElement>(".project-heading-trigger")!;
  await act(async () => trigger.focus());
  expect(container.querySelector(".project-hover-card")).not.toBeNull();
  await act(async () => trigger.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(container.querySelector(".project-hover-card")).toBeNull();
  await act(async () => trigger.click());
  await act(async () => { container.querySelector("header")!.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body })); vi.advanceTimersByTime(180); });
  expect(container.querySelector(".project-hover-card")).toBeNull();
});
it("opens an independent project menu, navigates with the keyboard and dispatches actions", async () => {
  const pin = vi.fn(), reveal = vi.fn().mockResolvedValue(undefined), remove = vi.fn(), create = vi.fn();
  await act(async () => root.render(<ProjectHoverCard project={project} onEdit={edit} onPin={pin} onReveal={reveal} onRemove={remove} onCreateConversation={create} />));
  const more = container.querySelector<HTMLButtonElement>('[aria-haspopup="menu"]')!;
  await act(async () => more.click());
  expect(container.querySelector('[role="menu"]')?.textContent).toContain("在文件管理器中查看");
  expect(container.querySelector(".project-hover-card")).toBeNull();
  expect(document.activeElement?.textContent).toBe("置顶项目");
  await act(async () => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })));
  expect(document.activeElement?.textContent).toBe("编辑项目");
  await act(async () => (document.activeElement as HTMLButtonElement).click());
  expect(edit).toHaveBeenCalledWith(project);
  expect(document.activeElement).toBe(more);
  await act(async () => more.click());
  await act(async () => [...container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(button => button.textContent?.includes("文件管理器"))!.click());
  expect(reveal).toHaveBeenCalledWith(project);
  expect(container.querySelector('[role="menu"]')).toBeNull();
  await act(async () => more.click());
  await act(async () => document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })));
  expect(container.querySelector('[role="menu"]')).toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="在 DrillHUD 中新建会话"]')!.click());
  expect(create).toHaveBeenCalledWith(project);
});
it("keeps reveal errors visible and closes the menu with Escape", async () => {
  await act(async () => root.render(<ProjectHoverCard project={project} onReveal={async () => { throw new Error("项目目录不存在"); }} />));
  const more = container.querySelector<HTMLButtonElement>('[aria-haspopup="menu"]')!;
  await act(async () => more.click());
  await act(async () => container.querySelector<HTMLButtonElement>('[role="menuitem"]')!.click());
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("项目目录不存在");
  await act(async () => container.querySelector('[role="menu"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(container.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(more);
});
