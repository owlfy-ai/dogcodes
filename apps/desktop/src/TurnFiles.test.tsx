// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { it, expect, vi } from "vitest";
import type { ArtifactMetadata, CoreClient } from "@dogcodes/protocol";
import { TurnFiles } from "./TurnFiles";
vi.mock("./artifactView", () => ({
  acceptArtifactChunk: () => new TextEncoder().encode('{"files":[]}'),
  verifyArtifactBytes: async () => {},
}));

it("keeps fast, cached, and slow empty history reads free of temporary rows", async () => {
  vi.useFakeTimers();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const host = document.createElement("div"), root = createRoot(host);
  const metadata: ArtifactMetadata = { artifactId: "art_test", payloadSha256: "test", byteLength: 12, mediaType: "application/json", createdAt: "2026-09-11T00:00:00Z", redactionRulesetVersion: 1, redactionCount: 0 };
  let finish!: (value: unknown) => void;
  const client = { readArtifact: vi.fn(() => new Promise(resolve => { finish = resolve; })) } as unknown as CoreClient;
  try {
    await act(async () => root.render(<TurnFiles taskId="tsk_test" metadata={metadata} client={client} />));
    expect(host.textContent).toBe("");
    await act(async () => vi.advanceTimersByTime(100));
    await act(async () => finish({}));
    await act(async () => vi.advanceTimersByTime(300));
    expect(host.textContent).toBe("");
    await act(async () => root.render(<TurnFiles key="second" taskId="tsk_test" metadata={metadata} client={client} />));
    expect(client.readArtifact).toHaveBeenCalledTimes(1);
    expect(host.querySelector('[role="status"]')).toBeNull();
    await act(async () => root.render(<TurnFiles key="slow" taskId="tsk_test" metadata={{ ...metadata, artifactId: "art_slow" }} client={client} />));
    await act(async () => vi.advanceTimersByTime(299));
    expect(host.textContent).toBe("");
    await act(async () => vi.advanceTimersByTime(5000));
    expect(host.childElementCount).toBe(0);
    await act(async () => finish({}));
    expect(host.textContent).toBe("");
  } finally {
    await act(async () => root.unmount()); vi.useRealTimers(); vi.unstubAllGlobals();
  }
});
