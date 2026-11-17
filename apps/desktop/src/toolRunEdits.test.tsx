// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ArtifactMetadata, CoreClient } from "@dogcodes/protocol";
import { editPatch } from "./conversationDiff";
import { useToolRunEdits } from "./toolRunEdits";
import type { TurnArtifact } from "./turnArtifactCache";

const { load } = vi.hoisted(() => ({ load: vi.fn() }));
vi.mock("./turnArtifactCache", () => ({
  cachedTurnArtifact: () => undefined,
  loadTurnArtifact: load,
  turnArtifactKey: (task: string, metadata: ArtifactMetadata) => `${task}:${metadata.artifactId}`,
}));
const metadata: ArtifactMetadata = { artifactId: "art_capture", mediaType: "application/json", payloadSha256: "hash",
  byteLength: 1, createdAt: "2026-11-15T00:00:00Z", redactionRulesetVersion: 1, redactionCount: 0 };
const client = {} as CoreClient;
const captured: TurnArtifact = { data: { files: [{ path: "a.ts", status: "M" }, { path: "b.ts", status: "A" }] },
  diff: { files: [], additions: 5, deletions: 2, truncated: false, truncationReason: null }, expanded: false };
let host: HTMLDivElement, root: ReturnType<typeof createRoot>;
function Preview({ sources }: { sources: Parameters<typeof useToolRunEdits>[0] }) {
  return <output>{JSON.stringify(useToolRunEdits(sources, client))}</output>;
}
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  load.mockReset(); host = document.createElement("div"); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); vi.unstubAllGlobals(); });
const stats = () => JSON.parse(host.textContent || "{}");

it("uses captured diffs once, deduplicates file paths and sums separate operations", async () => {
  let finish!: (value: TurnArtifact) => void;
  load.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const artifact = { taskId: "tsk_test" as const, artifact: metadata,
    files: [{ path: "a.ts", diff: editPatch("a.ts", "old\n", "reported\n") }] };
  await act(async () => root.render(<Preview sources={[artifact, artifact,
    { taskId: "tsk_test", files: [{ path: "a.ts", diff: editPatch("a.ts", "one\n", "two\n") }] }]} />));
  expect(load).toHaveBeenCalledTimes(1);
  expect(stats().partial).toBe(true);
  await act(async () => finish(captured));
  expect(stats()).toEqual({ files: 2, additions: 6, deletions: 3, hasDiff: true, partial: false });
});

it("does not leak a late artifact response into the next run", async () => {
  let finish!: (value: TurnArtifact) => void;
  load.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  await act(async () => root.render(<Preview sources={[{ taskId: "tsk_test", artifact: metadata, files: [] }]} />));
  await act(async () => root.render(<Preview sources={[]} />));
  await act(async () => finish(captured));
  expect(stats().files).toBe(0);
});

it("keeps unavailable or unparseable diffs out of known line totals", async () => {
  load.mockRejectedValue(new Error("artifact unavailable"));
  await act(async () => root.render(<Preview sources={[
    { taskId: "tsk_test", artifact: metadata, files: [] },
    { taskId: "tsk_test", files: [{ path: "binary.png" }] },
  ]} />));
  expect(stats()).toEqual({ files: 1, additions: 0, deletions: 0, hasDiff: false, partial: true });
});
