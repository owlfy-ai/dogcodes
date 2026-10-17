// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { WorkspaceFiles } from "./WorkspaceFiles";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class { onmessage = () => {}; } }));
it("loads multiple roots lazily and previews highlighted source", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  invoke.mockImplementation((command: string, args: { path?: string }) => {
    if (command === "core_workspace_roots") return Promise.resolve(["/one", "/two"]);
    if (command === "core_workspace_directory") return Promise.resolve(args.path === "src" ? [{ name: "a.ts", path: "src/a.ts", directory: false }] : [{ name: "src", path: "src", directory: true }]);
    return Promise.resolve({ text: "const x = 1;", image: null });
  });
  const container = document.createElement("div"); document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => root.render(<WorkspaceFiles taskId="task" />));
    expect(container.textContent).toContain("one"); expect(container.textContent).toContain("two");
    expect(invoke.mock.calls.filter(([command]) => command === "core_workspace_directory")).toHaveLength(0);
    await act(async () => (container.querySelector('[data-tooltip="one"]') as HTMLElement).click());
    await act(async () => (container.querySelector('[data-tooltip="two"]') as HTMLElement).click());
    await act(async () => (container.querySelector('[data-tooltip="src"]') as HTMLElement).click());
    await act(async () => (container.querySelector('[data-tooltip="src/a.ts"]') as HTMLElement).click());
    expect(invoke).toHaveBeenCalledWith("core_workspace_preview", { taskId: "task", root: 0, path: "src/a.ts" });
    expect(container.querySelector('.hljs-keyword')?.textContent).toBe("const");
    const watches = invoke.mock.calls.filter(([command]) => command === "core_workspace_watch");
    const srcWatch = watches.find(([, args]) => args.root === 0 && args.path === "src")![1];
    const before = invoke.mock.calls.filter(([command]) => command === "core_workspace_directory").length;
    await act(async () => srcWatch.changes.onmessage({ kind: "invalidated" }));
    expect(invoke.mock.calls.filter(([command]) => command === "core_workspace_directory")).toHaveLength(before + 1);
    expect(container.querySelector('[data-tooltip="src"]')?.getAttribute("aria-expanded")).toBe("true");
    await act(async () => root.render(<WorkspaceFiles taskId="task" visible={false} />));
    for (const [, args] of watches) expect(invoke).toHaveBeenCalledWith("core_workspace_unwatch", { id: args.id });
    const paused = invoke.mock.calls.length;
    await act(async () => srcWatch.changes.onmessage({ kind: "invalidated" }));
    expect(invoke.mock.calls).toHaveLength(paused);
    await act(async () => root.render(<WorkspaceFiles taskId="task" visible />));
    expect(invoke.mock.calls.filter(([command]) => command === "core_workspace_directory")).toHaveLength(before + 4);
    await act(async () => (container.querySelector('[aria-label="全部折叠"]') as HTMLElement).click());
    expect(container.querySelectorAll('.review-folder[aria-expanded="true"]')).toHaveLength(0);
    expect(container.querySelector('.hljs-keyword')?.textContent).toBe("const");
    await act(async () => (container.querySelector('[data-tooltip="one"]') as HTMLElement).click());
    expect(container.querySelector('[data-tooltip="src"]')?.getAttribute("aria-expanded")).toBe("false");

  } finally { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); }
});

it("renders Markdown files as formatted documents", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  invoke.mockImplementation((command: string) => {
    if (command === "core_workspace_roots") return Promise.resolve(["/repo"]);
    if (command === "core_workspace_directory") return Promise.resolve([{ name: "README.md", path: "README.md", directory: false }]);
    if (command === "core_workspace_preview") return Promise.resolve({ text: "# Project\n\n**Ready**\n\n- First\n- Second", image: null });
    return Promise.resolve();
  });
  const container = document.createElement("div"); document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => root.render(<WorkspaceFiles taskId="task" />));
    await act(async () => (container.querySelector('[data-tooltip="repo"]') as HTMLElement).click());
    await act(async () => (container.querySelector('[data-tooltip="README.md"]') as HTMLElement).click());
    expect(container.querySelector('.workspace-markdown h1')?.textContent).toBe("Project");
    expect(container.querySelector('.workspace-markdown strong')?.textContent).toBe("Ready");
    expect(container.querySelectorAll('.workspace-markdown li')).toHaveLength(2);
    expect(container.querySelector('.workspace-code')).toBeNull();
  } finally { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); }
});

it("reveals a requested file by expanding only its ancestors, including on repeated requests", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const scroll = vi.fn(); HTMLElement.prototype.scrollIntoView = scroll;
  invoke.mockImplementation((command: string, args: { path?: string }) => {
    if (command === "core_workspace_roots") return Promise.resolve(["/one", "/two"]);
    if (command === "core_workspace_directory") return Promise.resolve(args.path === "src" ? [{ name: "a.ts", path: "src/a.ts", directory: false }] : [{ name: "src", path: "src", directory: true }, { name: "other", path: "other", directory: true }]);
    if (command === "core_workspace_preview") return Promise.resolve({ text: "const x = 1;", image: null });
    return Promise.resolve();
  });
  const container = document.createElement("div"); document.body.append(container); const root = createRoot(container);
  try {
    await act(async () => root.render(<WorkspaceFiles taskId="task" reveal={{ path: "/two/src/a.ts", id: 1 }} />));
    expect(container.querySelector('[data-tooltip="one"]')?.getAttribute('aria-expanded')).toBe('false');
    expect(container.querySelector('[data-tooltip="two"]')?.getAttribute('aria-expanded')).toBe('true');
    expect(container.querySelector('[data-tooltip="src"]')?.getAttribute('aria-expanded')).toBe('true');
    expect(container.querySelector('[data-tooltip="other"]')?.getAttribute('aria-expanded')).toBe('false');
    expect(container.querySelector('[data-tooltip="src/a.ts"]')?.getAttribute('aria-pressed')).toBe('true');
    expect(invoke).toHaveBeenCalledWith('core_workspace_preview', { taskId: 'task', root: 1, path: 'src/a.ts' });
    expect(scroll).toHaveBeenCalled();
    await act(async () => (container.querySelector('[aria-label="全部折叠"]') as HTMLElement).click());
    await act(async () => root.render(<WorkspaceFiles taskId="task" reveal={{ path: "/two/src/a.ts", id: 2 }} />));
    expect(container.querySelector('[data-tooltip="src"]')?.getAttribute('aria-expanded')).toBe('true');
  } finally { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); }
});

it("toggles noise filtering, preserves source files and clears a hidden selection", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.removeItem("dogcodes.files.hideNoise");
  invoke.mockImplementation((command: string) => {
    if (command === "core_workspace_roots") return Promise.resolve(["/repo"]);
    if (command === "core_workspace_directory") return Promise.resolve([
      ...["node_modules", ".git", ".pnpm-store", ".claude", ".wrangler", "dist", "src"].map(name => ({ name, path: name, directory: true })),
      ...[".env", "package.json", "pnpm-lock.yaml", "README.md"].map(name => ({ name, path: name, directory: false })),
    ]);
    if (command === "core_workspace_preview") return Promise.resolve({ text: "example\n", image: null });
    return Promise.resolve();
  });
  const container = document.createElement("div"); const root = createRoot(container);
  try {
    await act(async () => root.render(<WorkspaceFiles taskId="task" />));
    await act(async () => container.querySelector<HTMLButtonElement>('[data-tooltip="repo"]')!.click());
    await act(async () => container.querySelector<HTMLButtonElement>('[data-tooltip=".env"]')!.click());
    expect(container.querySelector('.review-file-body')).not.toBeNull();
    const toggle = container.querySelector<HTMLButtonElement>('[aria-label="过滤干扰文件"]')!;
    expect(toggle.previousElementSibling?.getAttribute('aria-label')).toBe('全部折叠');
    await act(async () => toggle.click());
    expect(toggle.getAttribute('aria-pressed')).toBe('true');
    expect(localStorage.getItem('dogcodes.files.hideNoise')).toBe('true');
    for (const name of ['node_modules', '.git', '.pnpm-store', '.claude', '.wrangler', 'dist', '.env']) expect(container.querySelector(`[data-tooltip="${name}"]`)).toBeNull();
    for (const name of ['src', 'package.json', 'pnpm-lock.yaml', 'README.md']) expect(container.querySelector(`[data-tooltip="${name}"]`)).not.toBeNull();
    expect(container.querySelector('.review-file-body')).toBeNull();
    await act(async () => toggle.click());
    expect(container.querySelector('[data-tooltip=".env"]')).not.toBeNull();
    expect(container.querySelector('[data-tooltip="node_modules"]')).not.toBeNull();
  } finally { await act(async () => root.unmount()); localStorage.removeItem('dogcodes.files.hideNoise'); vi.unstubAllGlobals(); }
});

it("shows directory guidance after loading without starting an empty-root search", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  invoke.mockClear(); invoke.mockResolvedValue([]);
  const container = document.createElement("div"), root = createRoot(container);
  try {
    await act(async () => root.render(<WorkspaceFiles taskId="no-roots" />));
    expect(container.querySelector(".review-empty-state h3")?.textContent).toBe("尚未关联项目目录");
    expect(container.querySelector<HTMLInputElement>('input[type="search"]')?.disabled).toBe(true);
    expect(invoke).not.toHaveBeenCalledWith("core_workspace_find_files", expect.anything());
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});
