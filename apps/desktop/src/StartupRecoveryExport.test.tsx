// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true, invoke }));
import { StartupRecovery } from "./StartupGate";
let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  invoke.mockReset();
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
it.each(["saved", "cancelled"])("exports startup diagnostics without bootstrapping Core (%s)", async result => {
  invoke.mockImplementation(async command => {
    if (command === "core_startup_diagnostics") return { code: "host_lease_unavailable" };
    if (command === "core_export_startup_diagnostics") return result;
    throw new Error("healthy Core must not be called");
  });
  await act(async () => root.render(<StartupRecovery status="storage_recovery_required" />));
  expect(container.textContent).toContain("应用可能已在其他窗口运行");
  await act(async () => container.querySelector("button")!.click());
  expect(invoke.mock.calls.map(call => call[0])).toEqual(["core_startup_diagnostics", "core_export_startup_diagnostics"]);
  expect(container.querySelector(".startup-notice")?.textContent).toBe(result === "saved" ? "问题报告已保存。" : "");
});
it("restarts the application when the startup service check failed", async () => {
  invoke.mockResolvedValue(undefined);
  await act(async () => root.render(<StartupRecovery status="service_unavailable" />));
  const retry = container.querySelector<HTMLButtonElement>(".startup-actions button")!;
  expect(retry.textContent).toBe("重新打开应用");
  await act(async () => retry.click());
  expect(invoke).toHaveBeenCalledWith("restart_application");
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("keeps a recovery route when restarting the application fails", async () => {
  invoke.mockRejectedValue(new Error("IPC failed"));
  await act(async () => root.render(<StartupRecovery status="service_unavailable" />));
  await act(async () => container.querySelector<HTMLButtonElement>(".startup-actions button")!.click());
  expect(container.querySelector(".startup-notice")?.textContent).toBe("请退出应用后重新打开。");
  expect(container.querySelector<HTMLButtonElement>(".startup-actions button")!.disabled).toBe(false);
});
