// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { OpenFileButton } from "./OpenFileButton";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
let root: Root, host: HTMLDivElement;
const response = (applications = [{ id: "reader", name: "Reader", isDefault: true }] as Array<{ id: string; name: string; isDefault: boolean; icon?: string | null }>, error: string | null = null, absolutePath = "/docs/report.pdf") => ({ applications, error, absolutePath });
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  vi.mocked(invoke).mockReset().mockImplementation(async command => command === "core_workspace_file_applications" ? response() : undefined);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const render = (path = "/docs/report.pdf", disabled = false) => act(async () => root.render(<OpenFileButton taskId="tsk_test" path={path} disabled={disabled} />));
const click = (element: Element) => act(async () => (element as HTMLElement).click());
const trigger = () => host.querySelector<HTMLButtonElement>('[aria-haspopup="menu"]')!;
const icon = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a2ioAAAAASUVORK5CYII=";

it("shows native names and icons together only after the icons have decoded", async () => {
  let decoded!: () => void;
  vi.stubGlobal("Image", class { src = ""; decode() { return new Promise<void>(resolve => { decoded = resolve; }); } });
  vi.mocked(invoke).mockResolvedValueOnce(response([{ id: "preview", name: "预览", isDefault: true, icon }]));
  await render(); await click(trigger());
  expect(host.querySelector('[role="menu"]')).toBeNull();
  await act(async () => decoded());
  const item = host.querySelectorAll('[role="menuitem"]')[1];
  expect(item.textContent).toBe("预览默认");
  expect(item.querySelector('img')?.getAttribute('src')).toBe(icon);
  expect(item.querySelector('img')?.getAttribute('alt')).toBe("");
});

it("keeps application names and actions available when an icon is missing or cannot decode", async () => {
  vi.stubGlobal("Image", class { src = ""; decode() { return Promise.reject(new Error("invalid PNG")); } });
  vi.mocked(invoke).mockResolvedValueOnce(response([
    { id: "broken", name: "Reader", isDefault: false, icon },
    { id: "missing", name: "Viewer", isDefault: false, icon: null },
  ]));
  await render(); await click(trigger());
  expect(host.querySelectorAll('.open-file-app-placeholder')).toHaveLength(2);
  expect(host.querySelector('img')).toBeNull();
  await click(host.querySelectorAll('[role="menuitem"]')[1]);
  expect(invoke).toHaveBeenLastCalledWith("core_open_workspace_file", { taskId: "tsk_test", path: "/docs/report.pdf", applicationId: "broken" });
});

it("keeps default opening separate from querying and choosing an application", async () => {
  await render();
  expect(invoke).toHaveBeenCalledWith("core_workspace_file_applications", { taskId: "tsk_test", path: "/docs/report.pdf" });
  await click(host.querySelector('[aria-label="打开 report.pdf"]')!);
  expect(invoke).toHaveBeenLastCalledWith("core_open_workspace_file", { taskId: "tsk_test", path: "/docs/report.pdf" });
  await click(trigger());
  expect(host.textContent).toContain("Reader默认");
  await click(host.querySelectorAll('[role="menuitem"]')[1]);
  expect(invoke).toHaveBeenLastCalledWith("core_open_workspace_file", { taskId: "tsk_test", path: "/docs/report.pdf", applicationId: "reader" });
  expect(host.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(trigger());
});

it("supports keyboard navigation, Escape and outside dismissal", async () => {
  await render(); await click(trigger());
  const items = host.querySelectorAll<HTMLButtonElement>('[role="menuitem"]');
  expect(document.activeElement).toBe(items[0]);
  await act(async () => items[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })));
  expect(document.activeElement).toBe(items[1]);
  await act(async () => items[1].dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(host.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(trigger());
  await click(trigger());
  await act(async () => document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })));
  expect(host.querySelector('[role="menu"]')).toBeNull();
});

it("discards late results after switching files and preloads the new file", async () => {
  let finish!: (value: unknown) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  await render(); await click(trigger());
  await render("/docs/new.html");
  await act(async () => finish(response([{ id: "stale", name: "Stale reader", isDefault: false }])));
  expect(host.querySelector('[role="menu"]')).toBeNull();
  await click(trigger());
  expect(invoke).toHaveBeenCalledWith("core_workspace_file_applications", { taskId: "tsk_test", path: "/docs/new.html" });
  expect(host.textContent).not.toContain("Stale reader");
});

it("shows query failures with retry, and reports a removed application's launch failure", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(response([], "无法查询系统推荐应用")).mockResolvedValueOnce(response([], "无法查询系统推荐应用"));
  await render();
  await click(trigger());
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("无法查询");
  await click(host.querySelectorAll('[role="menuitem"]')[1]);
  vi.mocked(invoke).mockRejectedValueOnce("此应用已不可用，请重新打开应用列表");
  await click(host.querySelectorAll('[role="menuitem"]')[1]);
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("已不可用");
});

it("opens with complete results and reuses them without a loading frame or another query", async () => {
  let finish!: (value: unknown) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  await render(); await click(trigger());
  expect(trigger().getAttribute("aria-busy")).toBe("true");
  expect(host.querySelector('[role="menu"]')).toBeNull();
  expect(host.textContent).not.toContain("正在查找");
  await act(async () => finish(response()));
  expect(host.querySelector('[role="menu"]')?.textContent).toContain("Reader默认");
  expect(trigger().getAttribute("aria-busy")).toBeNull();
  await click(trigger()); await click(trigger());
  expect(host.querySelector('[role="menu"]')?.textContent).toContain("Reader默认");
  expect(invoke).toHaveBeenCalledTimes(1);
});

it.each(["Escape", "outside"])("does not reopen on late results after %s cancels a pending opening", async dismissal => {
  let finish!: (value: unknown) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  await render(); await click(trigger());
  await act(async () => {
    if (dismissal === "Escape") trigger().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    else document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
  });
  await act(async () => finish(response()));
  expect(host.querySelector('[role="menu"]')).toBeNull();
  await click(trigger());
  expect(host.textContent).toContain("Reader默认");
  expect(invoke).toHaveBeenCalledTimes(1);
});

it("keeps the default action when no recommended apps exist and disables deleted files", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(response([]));
  await render(); await click(trigger());
  expect(host.textContent).toContain("没有其他系统推荐的应用");
  expect(host.querySelectorAll('[role="menuitem"]')).toHaveLength(3);
  await render("/docs/report.pdf", true);
  expect(trigger().disabled).toBe(true);
  expect(host.querySelector('[role="menu"]')).toBeNull();
});


it("keeps reveal and absolute-path copy in the footer, including keyboard navigation", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  await render("report.pdf"); await click(trigger());
  const footer = host.querySelector('.open-file-footer')!;
  const items = footer.querySelectorAll<HTMLButtonElement>('[role="menuitem"]');
  expect(items).toHaveLength(2);
  expect(footer.closest('[data-popover-scroll]')).toBeNull();
  await act(async () => host.querySelector('[role="menu"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true })));
  expect(document.activeElement).toBe(items[1]);
  await click(items[1]);
  expect(writeText).toHaveBeenCalledWith("/docs/report.pdf");
  expect(host.querySelector('[role="menu"]')).toBeNull();
  await click(trigger()); await click(host.querySelector('.open-file-footer button')!);
  expect(invoke).toHaveBeenLastCalledWith("core_reveal_workspace_file", { taskId: "tsk_test", path: "report.pdf" });
});

it.each([
  ["MacIntel", "zh-CN", "在访达中显示"],
  ["MacIntel", "en-US", "在 Finder 中显示"],
  ["Win32", "zh-CN", "在文件资源管理器中显示"],
  ["Linux x86_64", "zh-CN", "在文件管理器中显示"],
])("uses the file manager name for %s / %s", async (platform, language, label) => {
  vi.spyOn(navigator, "platform", "get").mockReturnValue(platform);
  vi.spyOn(navigator, "language", "get").mockReturnValue(language);
  await render(); await click(trigger());
  expect(host.querySelector('.open-file-footer button')?.textContent).toBe(label);
});

it("keeps footer actions when application lookup fails and reports reveal failures", async () => {
  vi.mocked(invoke).mockResolvedValueOnce(response([], "查询失败")).mockResolvedValueOnce(response([], "查询失败"));
  await render(); await click(trigger());
  const footer = host.querySelector('.open-file-footer')!;
  expect(footer.textContent).toContain("复制路径");
  vi.mocked(invoke).mockRejectedValueOnce("文件不存在");
  await click(footer.querySelector('button')!);
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("文件不存在");
});


it("recovers a failed path query on reopening instead of permanently caching a disabled copy action", async () => {
  vi.mocked(invoke).mockRejectedValueOnce("文件不存在或不属于当前工作区").mockRejectedValueOnce("文件不存在或不属于当前工作区");
  await render(); await click(trigger());
  const copy = () => host.querySelector<HTMLButtonElement>('.open-file-footer button:last-child')!;
  expect(copy().disabled).toBe(true);
  expect(copy().dataset.tooltip).toContain("文件不存在");
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("文件不存在");
  await click(trigger()); await click(trigger());
  expect(copy().disabled).toBe(false);
  expect(host.textContent).toContain("Reader默认");
  expect(invoke).not.toHaveBeenCalledWith("core_workspace_file_path", expect.anything());
});

it("keeps copying enabled even when native application discovery fails", async () => {
  vi.mocked(invoke).mockResolvedValue(response([], "无法查询系统推荐应用"));
  await render(); await click(trigger());
  expect(host.querySelector<HTMLButtonElement>('.open-file-footer button:last-child')!.disabled).toBe(false);
  expect(host.textContent).toContain("无法查询系统推荐应用");
});
