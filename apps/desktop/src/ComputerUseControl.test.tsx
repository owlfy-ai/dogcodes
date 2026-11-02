// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import type { CoreClient } from "@dogcodes/protocol";
import { ComputerUseControl } from "./ComputerUseControl";

it("controls the host without a selected task and reloads persisted state when the menu reopens", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let enabled = true;
  const computerUse = vi.fn(async (call: { type: string; enabled?: boolean }) => {
    if (call.type === "set") enabled = call.enabled!;
    return { available: true, enabled, active: true, externalInput: false };
  });
  const client = { computerUse } as unknown as CoreClient;
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => root.render(<ComputerUseControl client={client} />));
    let button = container.querySelector<HTMLButtonElement>('[role="switch"]')!;
    expect(button.disabled).toBe(false);
    expect(button.getAttribute("aria-checked")).toBe("true");
    await act(async () => button.click());
    expect(computerUse).toHaveBeenLastCalledWith({ type: "set", enabled: false });
    expect(button.getAttribute("aria-checked")).toBe("false");
    await act(async () => button.click());
    expect(computerUse).toHaveBeenLastCalledWith({ type: "set", enabled: true });
    await act(async () => root.render(null));
    enabled = false;
    await act(async () => root.render(<ComputerUseControl client={client} />));
    button = container.querySelector<HTMLButtonElement>('[role="switch"]')!;
    expect(button.getAttribute("aria-checked")).toBe("false");
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});

it("shows failures and permits turning off an enabled preference even if the driver is unavailable", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const computerUse = vi.fn(async (call: {type: string}) => {
    if (call.type === "set") throw new Error("offline");
    return { available:false,enabled:true,active:false,externalInput:false };
  });
  const client = { computerUse } as unknown as CoreClient;
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => root.render(<ComputerUseControl client={client} />));
    const button = container.querySelector<HTMLButtonElement>("button")!;
    expect(button.disabled).toBe(false);
    await act(async () => button.click());
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("offline");
    expect(button.getAttribute("aria-checked")).toBe("true");
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});
