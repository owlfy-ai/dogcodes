// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";
it("recovers a failed bootstrap without losing the user in a loading screen", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  const client = new DemoCoreClient();
  const original = client.bootstrap.bind(client);
  const bootstrap = vi.spyOn(client, "bootstrap").mockRejectedValueOnce(new Error("IPC internal failure")).mockImplementation(original);
  const host = document.createElement("div"); document.body.append(host); const root = createRoot(host);
  try {
    await act(async () => root.render(<App client={client} />));
    expect(host.textContent).toContain("暂时无法打开对话");
    expect(host.textContent).not.toContain("IPC");
    await act(async () => host.querySelector<HTMLButtonElement>(".startup-actions button")!.click());
    expect(bootstrap.mock.calls.length).toBeGreaterThan(1);
    expect(host.querySelector(".startup-shell")).toBeNull();
    expect(host.querySelector("textarea")).not.toBeNull();
    expect(host.querySelector(".error")).toBeNull();
  } finally { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); }
});
