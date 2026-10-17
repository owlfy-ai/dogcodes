import type { ArtifactMetadata, CoreClient, TaskId } from "@dogcodes/protocol";
import { acceptArtifactChunk, verifyArtifactBytes } from "./artifactView";
import { frozenDiff } from "./conversationDiff";
import type { StructuredDiff } from "./diffView";

export interface TurnArtifact {
  data: { files: { path: string; status: string }[]; patch?: string; attribution?: string; recoveredAfterInterruption?: boolean };
  diff: StructuredDiff | null;
  expanded: boolean;
}
interface Entry { promise: Promise<TurnArtifact>; value?: TurnArtifact; size: number }
const caches = new WeakMap<CoreClient, Map<string, Entry>>();
const readQueues = new WeakMap<CoreClient, { active: number; pending: (() => void)[] }>();

// Cards load independently of the timeline. Keep large histories from issuing
// every artifact read at once, including reads started by newly opened details.
function scheduleRead(client: CoreClient, read: () => Promise<TurnArtifact>): Promise<TurnArtifact> {
  let queue = readQueues.get(client);
  if (!queue) { queue = { active: 0, pending: [] }; readQueues.set(client, queue); }
  const state = queue;
  return new Promise((resolve, reject) => {
    const start = async () => {
      state.active++;
      try { resolve(await read()); } catch (error) { reject(error); }
      finally { state.active--; state.pending.shift()?.(); }
    };
    if (state.active < 4) void start(); else state.pending.push(() => { void start(); });
  });
}
export const turnArtifactKey = (taskId: TaskId, metadata: ArtifactMetadata) => JSON.stringify([taskId, metadata.artifactId, metadata.payloadSha256, metadata.byteLength]);
function cacheFor(client: CoreClient) {
  let cache = caches.get(client);
  if (!cache) { cache = new Map(); caches.set(client, cache); }
  return cache;
}
export function cachedTurnArtifact(client: CoreClient, taskId: TaskId, metadata: ArtifactMetadata) {
  const cache = cacheFor(client), key = turnArtifactKey(taskId, metadata), entry = cache.get(key);
  if (entry) { cache.delete(key); cache.set(key, entry); }
  return entry?.value;
}
export function loadTurnArtifact(client: CoreClient, taskId: TaskId, metadata: ArtifactMetadata): Promise<TurnArtifact> {
  const cache = cacheFor(client), key = turnArtifactKey(taskId, metadata);
  const existing = cache.get(key);
  if (existing) return existing.promise;
  const promise = scheduleRead(client, async (): Promise<TurnArtifact> => {
    let bytes: Uint8Array = new Uint8Array();
    do {
      const chunk = await client.readArtifact({ taskId, artifactId: metadata.artifactId, expectedPayloadSha256: metadata.payloadSha256, offset: bytes.length, limit: 32768 });
      bytes = acceptArtifactChunk(metadata, bytes, chunk);
    } while (bytes.length < metadata.byteLength);
    await verifyArtifactBytes(bytes, metadata.payloadSha256);
    const data = JSON.parse(new TextDecoder().decode(bytes));
    if (!Array.isArray(data.files) || !data.files.every((file: { path?: unknown }) => file && typeof file.path === "string")) throw new Error("变更记录格式不可用");
    const result = frozenDiff(data);
    return { data, diff: result.kind === "structured" ? result.diff : null, expanded: false };
  });
  const entry: Entry = { size: metadata.byteLength, promise };
  entry.promise = promise.then(value => {
    entry.value = value;
    // Bound retained immutable snapshots; pending reads remain shared until settled.
    let total = [...cache.values()].reduce((size, item) => size + item.size, 0);
    for (const [oldKey, item] of cache) {
      if (cache.size <= 128 && total <= 32 * 1024 * 1024) break;
      if (oldKey !== key && item.value) { cache.delete(oldKey); total -= item.size; }
    }
    return value;
  }).catch(error => { if (cache.get(key) === entry) cache.delete(key); throw error; });
  cache.set(key, entry);
  return entry.promise;
}
