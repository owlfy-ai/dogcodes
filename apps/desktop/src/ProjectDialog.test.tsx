// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ProjectId } from "@dogcodes/protocol";
import { ProjectDialog } from "./ProjectDialog";
import { invoke } from "@tauri-apps/api/core";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
let root: Root, container: HTMLDivElement;
const client = { updateProject: vi.fn(), registerProject: vi.fn(), createTask: vi.fn() };
const onClose = vi.fn(), onReady = vi.fn();
const click = async (element: Element) => act(async () => { element.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
const button = (text: string) => [...container.querySelectorAll("button")].find(b => b.textContent === text)!;
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: {} });
  vi.clearAllMocks();
  client.registerProject.mockResolvedValue("project"); client.createTask.mockResolvedValue("task");
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
  await act(async () => root.render(<ProjectDialog client={client} onClose={onClose} onReady={onReady} />));
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); Reflect.deleteProperty(window, "__TAURI_INTERNALS__"); vi.unstubAllGlobals(); });
it("creates a named project with unique directories and a selectable primary directory", async () => {
  expect(document.activeElement).toBe(container.querySelector("[data-dialog-title]"));
  expect(button("创建项目").disabled).toBe(true);
  const input = container.querySelector("input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "Product");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  for (const directory of ["/frontend", "/backend", "/backend"]) {
    vi.mocked(invoke).mockResolvedValueOnce(directory); await click(button("＋ 添加文件夹"));
  }
  expect(container.querySelectorAll(".project-directory-row")).toHaveLength(2);
  await click(button("设为主目录"));
  await act(async () => { container.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
  expect(client.registerProject).toHaveBeenCalledWith("Product", "/backend", ["/frontend"]);
  expect(onReady).toHaveBeenCalledWith("project");
  expect(client.createTask).not.toHaveBeenCalled();
});
it("cancelled folder selection and closing never register a project", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(null); await click(button("＋ 添加文件夹"));
  expect(container.querySelectorAll(".project-directory-row")).toHaveLength(0);
  await click(button("取消")); expect(onClose).toHaveBeenCalledOnce(); expect(client.registerProject).not.toHaveBeenCalled();
});
it("removes directories and reports native picker failures without closing", async () => {
  vi.mocked(invoke).mockResolvedValueOnce("/frontend"); await click(button("＋ 添加文件夹"));
  await click(container.querySelector('[aria-label="移除 /frontend"]')!);
  expect(container.querySelectorAll(".project-directory-row")).toHaveLength(0);
  vi.mocked(invoke).mockRejectedValueOnce(new Error("permission denied")); await click(button("＋ 添加文件夹"));
  expect(container.textContent).toContain("没有访问权限"); expect(onClose).not.toHaveBeenCalled();
});

const project = {projectId: "prj_edit" as ProjectId, displayName: "Original", path: "/frontend", additionalDirectories: ["/backend"], taskCount: 3, streamVersion: 4};
const edit = async () => act(async () => root.render(<ProjectDialog key="edit" project={project} client={client} onClose={onClose} onReady={onReady} />));
it("edits the existing project name and directories without creating a project or conversation", async () => {
  await edit();
  expect(container.querySelector("h2")?.textContent).toBe("编辑项目");
  expect(container.querySelector("input")?.value).toBe("Original");
  expect(container.querySelectorAll(".project-directory-row")).toHaveLength(2);
  await act(async () => {
    const input = container.querySelector("input")!;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "Updated");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await click(container.querySelector('[aria-label="移除 /frontend"]')!);
  vi.mocked(invoke).mockResolvedValueOnce("/shared"); await click(button("＋ 添加文件夹"));
  await act(async () => { container.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
  expect(client.updateProject).toHaveBeenCalledWith(project, "Updated", "/backend", ["/shared"]);
  expect(onReady).toHaveBeenCalledWith(project.projectId);
  expect(client.registerProject).not.toHaveBeenCalled();
  expect(client.createTask).not.toHaveBeenCalled();
});
it("retains edits on save failure and requires at least one directory", async () => {
  await edit();
  client.updateProject.mockRejectedValueOnce(new Error("项目已被修改，请重新打开编辑窗口"));
  await act(async () => { container.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("项目已被修改");
  expect(onClose).not.toHaveBeenCalled(); expect(onReady).not.toHaveBeenCalled();
  expect(container.querySelectorAll(".project-directory-row")).toHaveLength(2);
  await click(container.querySelector('[aria-label="移除 /frontend"]')!);
  await click(container.querySelector('[aria-label="移除 /backend"]')!);
  expect(button("保存").disabled).toBe(true);
  await click(button("取消")); expect(onClose).toHaveBeenCalledOnce();
  expect(project.additionalDirectories).toEqual(["/backend"]);
});
