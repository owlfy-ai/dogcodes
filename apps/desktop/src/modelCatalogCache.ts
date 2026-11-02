import type { ExecutorRef, ModelCatalog, TaskId } from "@dogcodes/protocol";
const FAILURE_TTL_MS = 30_000;
export interface ModelCatalogScope { model?: string | null; taskId?: TaskId }
interface Priority { foreground: boolean }
/** Successful scopes last for the Core lifetime; refresh keeps visible values. */
export class ModelCatalogCache {
  private sequence = 0;
  private published = new Map<string, number>();
  private values = new Map<string, ModelCatalog>();
  private entries = new Map<string, { promise: Promise<ModelCatalog>; pending: boolean; retryAfter: number; error?: unknown; priority: Priority; sequence: number }>();
  private dirty = new Set<string>();
  private listeners = new Set<() => void>();
  private queue: Array<{ run: () => void; priority: Priority }> = [];
  private running = 0;
  constructor(
    private read: (executor: ExecutorRef, model: string | null, taskId?: TaskId, refresh?: boolean) => Promise<ModelCatalog>,
    private retryDelays: readonly number[] = [1500, 4000], private now: () => number = Date.now,
  ) {}
  key(executor: ExecutorRef, version: string | null, scope: ModelCatalogScope = {}) {
    return JSON.stringify([executor.providerId, executor.installationId, version, scope.model ?? null]);
  }
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private notify() { this.listeners.forEach(listener => listener()); }
  invalidate() { this.entries.clear(); this.values.forEach((_, key) => this.dirty.add(key)); this.notify(); }
  peek(executor: ExecutorRef, version: string | null, scope: ModelCatalogScope = {}, _stale = false) {
    const value = this.values.get(this.key(executor, version, scope));
    if (value) return value;
    const base = this.values.get(this.key(executor, version, {}));
    const option = base?.models.find(item => item.id === scope.model);
    return scope.model && base && (base.defaultModel === scope.model || !option || option.thinkingOptions.length > 0) ? base : undefined;
  }
  fallback(executor: ExecutorRef, version: string | null, scope: ModelCatalogScope) {
    return this.peek(executor, version, scope) ?? this.peek(executor, version);
  }
  error(executor: ExecutorRef, version: string | null, scope: ModelCatalogScope) { return this.entries.get(this.key(executor, version, scope))?.error; }
  private isDirty(executor: ExecutorRef, version: string | null, scope: ModelCatalogScope) {
    const key = this.key(executor, version, scope);
    return this.dirty.has(key) || (!this.values.has(key) && !!this.peek(executor, version, scope)
      && this.dirty.has(this.key(executor, version, {})));
  }
  needsRead(executor: ExecutorRef, version: string | null, scope: ModelCatalogScope) {
    const key = this.key(executor, version, scope), entry = this.entries.get(key);
    if (entry?.error && this.now() < entry.retryAfter) return false;
    return this.isDirty(executor, version, scope)
      || !this.peek(executor, version, scope) || !!entry?.pending || (!!entry?.error && this.now() >= entry.retryAfter);
  }
  warm(executor: ExecutorRef, version: string | null) { return this.get(executor, version, false, {}, true); }
  get(executor: ExecutorRef, version: string | null, refresh = false, scope: ModelCatalogScope = {}, background = false): Promise<ModelCatalog> {
    const key = this.key(executor, version, scope), existing = this.entries.get(key);
    if (existing?.pending) { if (!background) existing.priority.foreground = true; return existing.promise; }
    if (!refresh && existing?.error && this.now() < existing.retryAfter) return existing.promise;
    if (!refresh && !this.isDirty(executor, version, scope)) {
      const cached = this.peek(executor, version, scope);
      if (cached && !existing?.error) return Promise.resolve(cached);
    }
    const priority = { foreground: !background };
    const entry = { promise: this.readWithRetry(executor, scope, refresh, priority), pending: true, retryAfter: 0, error: undefined as unknown, priority, sequence: ++this.sequence };
    this.entries.set(key, entry);
    void entry.promise.then(catalog => {
      if (this.entries.get(key) !== entry) return;
      entry.pending = false;
      const baseKey = this.key(executor, version);
      if (entry.sequence < (this.published.get(baseKey) ?? 0)) return;
      this.published.set(baseKey, entry.sequence);
      const previous = this.values.get(baseKey);
      const merged = { ...catalog, models: catalog.models.map(option => {
        const old = previous?.models.find(item => item.id === option.id);
        return old && option.id !== (scope.model ?? catalog.defaultModel) && !option.thinkingOptions.length
          ? { ...option, thinkingOptions: old.thinkingOptions, defaultThinking: old.defaultThinking } : option;
      }) };
      const prefix = JSON.stringify([executor.providerId, executor.installationId, version]).slice(0, -1) + ',';
      this.values.forEach((_, stored) => { if (stored.startsWith(prefix)) this.values.set(stored, merged); });
      this.values.set(baseKey, merged); this.values.set(key, merged);
      this.dirty.delete(baseKey); this.dirty.delete(key); this.notify();
    }, error => {
      if (this.entries.get(key) !== entry) return;
      entry.pending = false; entry.error = error; entry.retryAfter = this.now() + FAILURE_TTL_MS; this.notify();
    });
    return entry.promise;
  }
  private async readWithRetry(executor: ExecutorRef, scope: ModelCatalogScope, refresh: boolean, priority: Priority): Promise<ModelCatalog> {
    for (let attempt = 0; ; attempt++) {
      try {
        return await new Promise<ModelCatalog>((resolve, reject) => {
          this.queue.push({ priority, run: () => {
            this.running++;
            void Promise.resolve().then(() => refresh ? this.read(executor, scope.model ?? null, undefined, true)
              : this.read(executor, scope.model ?? null, undefined)).then(resolve, reject).finally(() => { this.running--; this.pump(); });
          } });
          this.pump();
        });
      } catch (error) {
        if (attempt >= this.retryDelays.length) throw error;
        await new Promise(resolve => setTimeout(resolve, this.retryDelays[attempt]));
      }
    }
  }
  private pump() {
    while (this.running < 2 && this.queue.length) {
      const index = this.queue.findIndex(job => job.priority.foreground);
      this.queue.splice(index < 0 ? 0 : index, 1)[0].run();
    }
  }
}
