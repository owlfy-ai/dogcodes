// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import type { CoreClient } from "@dogcodes/protocol";
import { ComputerUseControl } from "./ComputerUseControl";

it("allows stopping computer use during a run but requires an idle task to enable it", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let enabled = true;
  const computerUse = vi.fn(async (_task: string, call: { type: string; enabled?: boolean }) => {
    if (call.type === "set") enabled = call.enabled!;
    return { available: true, enabled, active: enabled, externalInput: false };
  });
  const client = { computerUse } as unknown as CoreClient;
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => root.render(<ComputerUseControl client={client} taskId="tsk_fixture" running />));
    let button = container.querySelector("button")!;
    expect(button.disabled).toBe(false);
    expect(button.getAttribute("aria-pressed")).toBe("true");
    await act(async () => button.click());
    expect(computerUse).toHaveBeenLastCalledWith("tsk_fixture", { type: "set", enabled: false });
    button = container.querySelector("button")!;
    expect(button.disabled).toBe(true);
    await act(async () => root.render(<ComputerUseControl client={client} taskId="tsk_fixture" running={false} />));
    expect(container.querySelector("button")!.disabled).toBe(false);
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});

it("does not apply an old task's status response after navigation", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  let oldReply: (value: unknown) => void = () => {};
  const computerUse = vi.fn((task: string) => task === "tsk_old" ? new Promise(resolve => { oldReply = resolve; }) : Promise.resolve({ available:true,enabled:false,active:false,externalInput:false }));
  const client = { computerUse } as unknown as CoreClient;
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => root.render(<ComputerUseControl client={client} taskId="tsk_old" running={false} />));
    await act(async () => root.render(<ComputerUseControl client={client} taskId="tsk_new" running={false} />));
    await act(async () => oldReply({ available:true,enabled:true,active:true,externalInput:false }));
    expect(container.querySelector("button")!.getAttribute("aria-pressed")).toBe("false");
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});
