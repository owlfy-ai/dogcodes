// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ProjectId } from "@dogcodes/protocol";
import { ProjectPicker } from "./ProjectPicker";
import { openLocalProject } from "./localProject";
import { TauriCoreClient } from "./tauriClient";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn(() => false) }));
let root: Root;
let container: HTMLDivElement;
const onSelect = vi.fn();
const project = {additionalDirectories: [],  projectId: "prj_one" as ProjectId, displayName: "Project", path: "/projects/one", taskCount: 0 };
const executor = { providerId: "codex", installationId: "ins_original" };
const click = async (element: Element) => {
  await act(async () => { element.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
};
const openMenu = async () => {
  await click(container.querySelector("button")!);
  expect(document.activeElement).toBe(container.querySelector("input"));
};

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.clearAllMocks();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});

async function render(onOpenFolder = vi.fn()) {
  await act(async () => root.render(<ProjectPicker projects={[project]} selectedProject={project}
    disabled={false} onSelect={onSelect} onOpenFolder={onOpenFolder} />));
}

describe("native project picker interactions", () => {
  it("survives a Safari null-relatedTarget blur and invokes native selection, registration and task creation", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "core_pick_project_directory") return "/projects/selected";
      if (command === "core_register_local_project") return "prj_selected";
      if (command === "core_execute") return {};
      throw new Error(`Unexpected native call: ${command}`);
    });
    const opened = vi.fn(() => openLocalProject(new TauriCoreClient(), executor));
    await render(opened);
    await openMenu();
    // macOS WKWebView pointer-clicking a button blurs the input without
    // transferring focus to the button, before delivering the click event.
    await act(async () => {
      container.querySelector("input")!.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: null }));
    });
    const folder = [...container.querySelectorAll("button")].find(button => button.textContent?.includes("创建项目"));
    expect(folder).toBeDefined();
    await click(folder!);
    expect(opened).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenNthCalledWith(1, "core_pick_project_directory");
    expect(invoke).toHaveBeenNthCalledWith(2, "core_register_local_project", { displayName: "", path: "/projects/selected" });
    expect(invoke).toHaveBeenNthCalledWith(3, "core_execute", expect.objectContaining({
      envelope: expect.objectContaining({ payload: {
        type: "create_task", data: expect.objectContaining({ projectId: "prj_selected", title: "新对话", preferredExecutor: executor }),
      } }),
    }));
    expect(container.querySelector('[role="dialog"]')).toBeNull();
  });

  it("still closes on keyboard focus outside and on outside pointerdown", async () => {
    await render();
    await openMenu();
    await act(async () => {
      container.querySelector("input")!.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: document.body }));
    });
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    await openMenu();
    await act(async () => { document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })); });
    expect(container.querySelector('[role="dialog"]')).toBeNull();
  });

  it("does not register or create a task when the native dialog is cancelled", async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    expect(await openLocalProject(new TauriCoreClient(), executor)).toBeNull();
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("does not create a task after failed directory validation", async () => {
    vi.mocked(invoke).mockResolvedValueOnce("/missing").mockRejectedValueOnce("无法打开项目目录");
    await expect(openLocalProject(new TauriCoreClient(), executor)).rejects.toBe("无法打开项目目录");
    expect(invoke).toHaveBeenCalledTimes(2);
  });
});
