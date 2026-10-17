import type { ExecutorRef, ModelCatalog, TaskId } from "@dogcodes/protocol";

export const MODEL_CATALOG_TTL_MS = 5 * 60_000;
const FAILURE_TTL_MS = 30_000;
export interface ModelCatalogScope { model?: string | null; taskId?: TaskId }

/** Bounded native queries, isolated by installation, version, workspace task and model. */
export class ModelCatalogCache {
  private values = new Map<string, { catalog: ModelCatalog; freshUntil: number }>();
  private entries = new Map<string, { promise: Promise<ModelCatalog>; pending: boolean; freshUntil: number }>();
  private queue: Array<() => void> = [];
  private running = 0;
  constructor(
    private read: (executor: ExecutorRef, model: string | null, taskId?: TaskId) => Promise<ModelCatalog>,
    private retryDelays: readonly number[] = [1500, 4000],
    private now: () => number = Date.now,
  ) {}
  key(executor: ExecutorRef, version: string | null, scope: ModelCatalogScope = {}) {
    return JSON.stringify([executor.providerId, executor.installationId, version, scope.taskId ?? null, scope.model ?? null]);
  }
  peek(executor: ExecutorRef, version: string | null, scope: ModelCatalogScope = {}, stale = false) {
    const key = this.key(executor, version, scope);
    if (!stale && this.entries.get(key)?.pending) return undefined;
    const value = this.values.get(key);
    return value && (stale || this.now() < value.freshUntil) ? value.catalog : undefined;
  }
  get(executor: ExecutorRef, version: string | null, refresh = false, scope: ModelCatalogScope = {}): Promise<ModelCatalog> {
    const key = this.key(executor, version, scope);
    const existing = this.entries.get(key);
    if (existing && (existing.pending || (!refresh && this.now() < existing.freshUntil))) return existing.promise;
    // Keep the last successful value visible during refresh and on failure.
    const entry = { promise: this.readWithRetry(executor, scope), pending: true, freshUntil: 0 };
    this.entries.set(key, entry);
    void entry.promise.then(catalog => {
      entry.pending = false;
      entry.freshUntil = this.now() + MODEL_CATALOG_TTL_MS;
      this.values.set(key, { catalog, freshUntil: entry.freshUntil });
    }, () => {
      entry.pending = false;
      entry.freshUntil = this.now() + FAILURE_TTL_MS;
      const value = this.values.get(key);
      if (value) value.freshUntil = 0;
    });
    return entry.promise;
  }
  private async readWithRetry(executor: ExecutorRef, scope: ModelCatalogScope): Promise<ModelCatalog> {
    for (let attempt = 0; ; attempt++) {
      try {
        return await new Promise<ModelCatalog>((resolve, reject) => {
          this.queue.push(() => {
            this.running++;
            void Promise.resolve().then(() => this.read(executor, scope.model ?? null, scope.taskId)).then(resolve, reject).finally(() => {
              this.running--;
              this.pump();
            });
          });
          this.pump();
        });
      } catch (error) {
        if (attempt >= this.retryDelays.length) throw error;
        await new Promise(resolve => setTimeout(resolve, this.retryDelays[attempt]));
      }
    }
  }
  private pump() { while (this.running < 2 && this.queue.length) this.queue.shift()!(); }
}
