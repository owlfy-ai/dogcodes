import type { ArtifactMetadata, CoreClient, TaskId } from "@dogcodes/protocol";
import { acceptArtifactChunk, verifyArtifactBytes } from "./artifactView";
import { frozenDiff, capturedFileDiff, type CapturedFile } from "./conversationDiff";
import type { StructuredDiff, DiffFile } from "./diffView";
import { translate as t } from "./i18n";

export interface TurnArtifact {
  data: {
    files: CapturedFile[];
    patch?: string; patchTruncated?: boolean; attribution?: string; recoveredAfterInterruption?: boolean; sharedWorkspace?: boolean;
    backend?: string; roots?: { workspacePath?: string; changes?: { backend?: string } }[];
  };
  diff: StructuredDiff | null;
  expanded: boolean;
}
interface Entry { promise: Promise<TurnArtifact>; value?: TurnArtifact; size: number }
const caches = new WeakMap<CoreClient, Map<string, Entry>>();
const readQueues = new WeakMap<CoreClient, { active: number; pending: (() => void)[] }>();

// Cards load independently of the timeline. Keep large histories from issuing
// every artifact read at once, including reads started by newly opened details.
function scheduleRead<T>(client: CoreClient, read: () => Promise<T>): Promise<T> {
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
    if (!Array.isArray(data.files) || !data.files.every((file: { path?: unknown }) => file && typeof file.path === "string")) throw new Error(t("turnArtifacts.badFormat"));
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

/** Fetch only the chosen file's immutable versions; never substitute live files. */
const fileCaches = new WeakMap<CoreClient, Map<string, { promise: Promise<DiffFile>; size: number; settled: boolean }>>();
export function loadTurnFile(client: CoreClient, taskId: TaskId, artifact: TurnArtifact, path: string): Promise<DiffFile> {
  const original = artifact.data.files.find(file => file.path === path);
  if (!original) return Promise.reject(new Error(t("turnArtifacts.badFormat")));
  let cache = fileCaches.get(client);
  if (!cache) { cache = new Map(); fileCaches.set(client, cache); }
  const key = JSON.stringify([taskId, path, original.before, original.after]);
  const existing = cache.get(key);
  if (existing) { cache.delete(key); cache.set(key, existing); return existing.promise; }
  const promise = scheduleRead(client, async () => {
    const file: CapturedFile = { ...original };
    const bodies = new Map<string, Uint8Array>();
    for (const side of ["before", "after"] as const) {
      const version = file[side], metadata = version?.contentArtifact;
      if (!metadata) continue;
      if (metadata.mediaType !== "application/vnd.dogcodes.workspace-content" || metadata.byteLength > 512 * 1024
        || metadata.payloadSha256 !== version.sha256) throw new Error(t("turnArtifacts.badFormat"));
      let bytes = bodies.get(metadata.payloadSha256);
      if (!bytes) {
        bytes = new Uint8Array();
        do {
          bytes = acceptArtifactChunk(metadata, bytes, await client.readArtifact({ taskId, artifactId: metadata.artifactId,
            expectedPayloadSha256: metadata.payloadSha256, offset: bytes.length, limit: 32768 }));
        } while (bytes.length < metadata.byteLength);
        await verifyArtifactBytes(bytes, metadata.payloadSha256);
        bodies.set(metadata.payloadSha256, bytes);
      }
      file[side] = { ...version, contentArtifact: undefined, hex: Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("") };
    }
    return capturedFileDiff(file);
  });
  const state = cache;
  // Include decoded strings and parsed lines in the estimate, not just manifest bytes.
  const size = [original.before, original.after].reduce((total, version) => total +
    (version?.contentArtifact?.byteLength ?? (version?.hex?.length ?? 0) / 2) * 8, 0);
  const entry = { promise, size, settled: false };
  state.set(key, entry);
  void promise.then(() => {
    entry.settled = true;
    let total = [...state.values()].reduce((sum, item) => sum + item.size, 0);
    for (const [oldKey, item] of state) {
      if (state.size <= 128 && total <= 32 * 1024 * 1024) break;
      if (item.settled) { state.delete(oldKey); total -= item.size; }
    }
  }, () => { state.delete(key); });
  return promise;
}
