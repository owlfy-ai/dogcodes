// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { StartupShell } from "./StartupShell";
import { AppErrorBoundary } from "./AppErrorBoundary";
import { userFacingError } from "./userFacingError";
let root: Root, container: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.useFakeTimers();
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
  localStorage.clear(); document.documentElement.removeAttribute("data-theme");
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
it("keeps ordinary startup quiet and provides recovery for a long wait", async () => {
  await act(async () => root.render(<StartupShell />));
  expect(container.textContent).not.toMatch(/Core|正在|启动|服务/);
  expect(container.querySelector("button")).toBeNull();
  await act(async () => vi.advanceTimersByTime(12000));
  expect(container.querySelector("button")?.textContent).toBe("重新打开");
  await act(async () => root.unmount()); root = createRoot(container);
  expect(vi.getTimerCount()).toBe(0);
});
it("restores explicit theme and layout before the application bundle runs", () => {
  localStorage.setItem("dogcodes.ui.theme", "dark");
  localStorage.setItem("dogcodes.ui.scale", "110");
  localStorage.setItem("dogcodes.sidebar.collapsed", "true");
  localStorage.setItem("dogcodes.sidebar.width", "320");
  window.eval(readFileSync("public/appearance.js", "utf8"));
  expect(document.documentElement.dataset.theme).toBe("dark");
  expect(document.documentElement.dataset.sidebarCollapsed).toBe("true");
  expect(document.documentElement.style.getPropertyValue("--ui-scale")).toBe("1.1");
  expect(document.documentElement.style.getPropertyValue("--startup-sidebar-width")).toBe("320px");
});
it("recovers from a rendering failure without exposing a stack trace", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  function Broken(): never { throw new Error("TypeError: internal implementation"); }
  await act(async () => root.render(<AppErrorBoundary><Broken /></AppErrorBoundary>));
  expect(container.textContent).toContain("页面暂时无法显示");
  expect(container.textContent).not.toContain("TypeError");
  expect(container.querySelector("button")?.textContent).toBe("重新打开");
});
it("keeps useful errors while translating operating-system and transport failures", () => {
  expect(userFacingError(new Error("EACCES: /private/path"), "未完成")).toContain("访问权限");
  expect(userFacingError({ internal: "secret" }, "未完成")).toBe("未完成");
  expect(userFacingError("请选择一个项目文件夹", "未完成")).toBe("请选择一个项目文件夹");
  expect(userFacingError("RPC {\"error\":\"内部故障\"}", "未完成")).toBe("未完成");
});
