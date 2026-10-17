// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { act, useEffect } from "react";
import { createRoot } from "react-dom/client";
import type { CoreClient } from "@dogcodes/protocol";
import { useModelReadyExecutors } from "./useModelReadyExecutors";
it("waits for runtime registration and updates after a late ready event", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div");
  const root = createRoot(container);
  let ready = false;
  let notify = () => {};
  const stop = vi.fn();
  const client = {
    modelReadyExecutors: vi.fn(async () => ready ? [{ providerId: "cursor", installationId: "ins_cursor" }] : []),
    onQuotaProviderReady: async (callback: () => void) => { notify = callback; return stop; },
  } as unknown as CoreClient;
  const readModels = vi.fn();
  function Probe() {
    const ids = useModelReadyExecutors(client);
    const key = ids.join(",");
    useEffect(() => { if (ids.includes("cursor:ins_cursor")) readModels(); }, [key]);
    return <span>{key}</span>;
  }
  try {
    await act(async () => root.render(<Probe />));
    expect(container.textContent).toBe("");
    expect(readModels).not.toHaveBeenCalled();
    // Persisted installation status never changes; only registration does.
    ready = true;
    await act(async () => notify());
    expect(container.textContent).toBe("cursor:ins_cursor");
    expect(readModels).toHaveBeenCalledOnce();
    await act(async () => notify());
    expect(readModels).toHaveBeenCalledOnce();
  } finally {
    await act(async () => root.unmount());
    expect(stop).toHaveBeenCalledOnce();
    vi.unstubAllGlobals();
  }
});
