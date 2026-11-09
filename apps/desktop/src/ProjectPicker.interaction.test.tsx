// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ProjectId } from "@dogcodes/protocol";
import { ProjectPicker } from "./ProjectPicker";
import type { TargetApi, TargetEntry } from "./executionTargetClient";
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

  it("groups projects under this computer and each host, keeping paths for the tooltip", async () => {
    const remote = { ...project, projectId: "prj_remote" as ProjectId, displayName: "Logs", path: "/root/logs", targetId: "tgt_one" };
    const list = vi.fn(async () => [{ target: { targetId: "tgt_one", displayName: "Build", connection: { destination: "192.168.1.100", auth: "system" } } }]) as unknown as TargetApi["list"];
    await act(async () => root.render(<ProjectPicker projects={[project, remote]} selectedProject={project}
      disabled={false} onSelect={onSelect} onOpenFolder={vi.fn()} targetApi={{ list }} />));
    await openMenu();
    const [local, server] = [...container.querySelectorAll<HTMLButtonElement>(".project-picker-list button")];
    expect(local.querySelector("small")).toBeNull();
    expect(local.textContent).not.toContain("/projects/one");
    expect(local.dataset.tooltip).toBe("/projects/one");
    const titles = [...container.querySelectorAll(".project-picker-group-title")].map(title => title.textContent);
    expect(titles).toEqual(["本机", "Build192.168.1.100"]);
    expect(server.closest('[role="group"]')?.getAttribute("aria-label")).toBe("Build");
    expect(server.querySelector("small")).toBeNull();
    expect(server.textContent).not.toContain("/root/logs");
    expect(server.dataset.tooltip).toBe("/root/logs");
  });

  it("still groups a remote project when the connection list cannot be read", async () => {
    const remote = { ...project, projectId: "prj_remote" as ProjectId, targetId: "tgt_one" };
    await act(async () => root.render(<ProjectPicker projects={[remote]} selectedProject={remote}
      disabled={false} onSelect={onSelect} onOpenFolder={vi.fn()} targetApi={{ list: async () => { throw new Error("offline"); } }} />));
    await openMenu();
    expect(container.querySelector(".project-picker-group-title")?.textContent).toBe("远程主机");
  });

  describe("remote hosts menu", () => {
    const host = { target: { targetId: "tgt_one", displayName: "Build", connection: { destination: "build.example.com", user: "dev", auth: "system" as const } }, status: {} as TargetEntry["status"], agents: [] };
    const renderHosts = async (entries: TargetEntry[]) => {
      const onChooseHost = vi.fn();
      const onNewHost = vi.fn();
      await act(async () => root.render(<ProjectPicker projects={[project]} selectedProject={project} disabled={false} onSelect={onSelect} onOpenFolder={vi.fn()}
        onChooseHost={onChooseHost} onNewHost={onNewHost} targetApi={{ list: vi.fn(async () => entries) }} />));
      await openMenu();
      await act(async () => {});
      return { onChooseHost, onNewHost };
    };
    const opener = () => [...container.querySelectorAll<HTMLButtonElement>(".project-picker-hosts > button")][0]!;
    const items = () => [...container.querySelectorAll<HTMLButtonElement>('[role="menu"] [role="menuitem"]')];

    it("replaces the new connection entry, opening its menu on hover", async () => {
      await renderHosts([host]);
      expect(opener().textContent).toBe("远程主机");
      expect(items()).toHaveLength(0);
      await act(async () => { opener().dispatchEvent(new MouseEvent("mouseover", { bubbles: true })); });
      expect(items().map(item => item.textContent)).toEqual(["Builddev@build.example.com", "新建远程主机"]);
    });

    it("hands over the chosen host and closes", async () => {
      const { onChooseHost } = await renderHosts([host]);
      await click(opener());
      await click(items()[0]!);
      expect(onChooseHost).toHaveBeenCalledWith(host);
      expect(container.querySelector(".project-picker-menu")).toBeNull();
    });

    it("always ends with a new host, even with none yet", async () => {
      const { onNewHost } = await renderHosts([]);
      await click(opener());
      expect(items().map(item => item.textContent)).toEqual(["新建远程主机"]);
      await click(items()[0]!);
      expect(onNewHost).toHaveBeenCalled();
    });

    it("closes the menu when the pointer moves on to another entry", async () => {
      await renderHosts([host]);
      await click(opener());
      await act(async () => { container.querySelector(".project-picker-list button")!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })); });
      expect(items()).toHaveLength(0);
    });
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
