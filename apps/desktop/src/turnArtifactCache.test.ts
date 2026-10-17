import { createHash } from "node:crypto";
import { expect, it, vi } from "vitest";
import type { ArtifactMetadata, CoreClient } from "@dogcodes/protocol";
import { cachedTurnArtifact, loadTurnArtifact } from "./turnArtifactCache";

function fixture() {
  const bytes = new TextEncoder().encode(JSON.stringify({ files: [{ path: "src/a.ts", status: "M", before: { hex: "61" }, after: { hex: "62" } }] }));
  const metadata: ArtifactMetadata = { artifactId: "art_test", payloadSha256: createHash("sha256").update(bytes).digest("hex"), byteLength: bytes.length, mediaType: "application/json", createdAt: "2026-09-13T00:00:00Z", redactionRulesetVersion: 1, redactionCount: 0 };
  const read = vi.fn(async () => ({ metadata, bytes: [...bytes], offset: 0, nextOffset: null }));
  const client = { readArtifact: read } as unknown as CoreClient;
  return { metadata, client, read };
}
it("shares pending and verified snapshots, including expansion state", async () => {
  const { client, metadata, read } = fixture();
  const first = loadTurnArtifact(client, "tsk_one", metadata);
  expect(loadTurnArtifact(client, "tsk_one", { ...metadata })).toBe(first);
  const snapshot = await first;
  expect(snapshot.diff?.additions).toBe(1);
  expect(snapshot.expanded).toBe(false);
  snapshot.expanded = true;
  expect(await loadTurnArtifact(client, "tsk_one", metadata)).toBe(snapshot);
  expect(cachedTurnArtifact(client, "tsk_one", metadata)?.expanded).toBe(true);
  expect(read).toHaveBeenCalledTimes(1);
  await loadTurnArtifact(client, "tsk_two", metadata);
  expect(read).toHaveBeenCalledTimes(2);
});
it("does not reuse another hash and permits retry after verification failure", async () => {
  const { client, metadata, read } = fixture();
  await loadTurnArtifact(client, "tsk_one", metadata);
  const changed = { ...metadata, payloadSha256: "invalid" };
  await expect(loadTurnArtifact(client, "tsk_one", changed)).rejects.toThrow();
  expect(cachedTurnArtifact(client, "tsk_one", changed)).toBeUndefined();
  await expect(loadTurnArtifact(client, "tsk_one", changed)).rejects.toThrow();
  expect(read).toHaveBeenCalledTimes(3);
  expect(cachedTurnArtifact(client, "tsk_one", metadata)).toBeDefined();
});
it("bounds independent card reads and releases queued work after failures", async () => {
  const { client, metadata, read } = fixture();
  const original = read.getMockImplementation()!;
  const release: (() => void)[] = [];
  read.mockImplementation(() => new Promise(resolve => { release.push(() => { void original().then(resolve); }); }));
  const reads = Array.from({ length: 8 }, (_, index) => loadTurnArtifact(client, `tsk_${index}`, metadata));
  // A duplicate must share the queued entry too.
  expect(loadTurnArtifact(client, "tsk_7", metadata)).toBe(reads[7]);
  expect(read).toHaveBeenCalledTimes(4);
  release.shift()!();
  await reads[0];
  expect(read).toHaveBeenCalledTimes(5);
  for (let index = 1; index < reads.length; index++) {
    release.shift()!();
    await reads[index];
  }
  expect(read).toHaveBeenCalledTimes(8);

  const reject: ((error: Error) => void)[] = [];
  read.mockImplementation(() => new Promise((_, fail) => { reject.push(fail); }));
  const failures = Array.from({ length: 5 }, (_, index) =>
    loadTurnArtifact(client, `tsk_failed_${index}`, metadata).catch(error => error));
  expect(reject).toHaveLength(4);
  for (let index = 0; index < failures.length; index++) {
    reject[index](new Error("read failed"));
    expect(await failures[index]).toBeInstanceOf(Error);
  }
  expect(reject).toHaveLength(5);
  read.mockImplementation(original);
  expect((await loadTurnArtifact(client, "tsk_failed_0", metadata)).data.files).toHaveLength(1);
});
