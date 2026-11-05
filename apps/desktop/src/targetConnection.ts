import { useSyncExternalStore } from "react";
import type { TargetApi, TargetEntry, TargetStatus } from "./executionTargetClient";

const IDLE_MILLISECONDS = 3000;
const RECONNECTING_MILLISECONDS = 400;

export interface TargetConnectionState {
  status: TargetStatus | null;
  /** Null until the first answer, so nothing is shown as offline before it is known. */
  offline: boolean | null;
  reconnecting: boolean;
  /** Which view started the attempt: the one that shows its dialog. */
  asker: string | null;
  entry: TargetEntry | null;
  failure: unknown;
}

const initial: TargetConnectionState = { status: null, offline: null, reconnecting: false, asker: null, entry: null, failure: null };

/** One per machine, however many views show it, so they can never disagree. */
class TargetConnection {
  private state = initial;
  private readonly listeners = new Set<() => void>();
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly api: TargetApi, private readonly targetId: string) {}

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    if (this.listeners.size === 1) void this.read();
    return () => {
      this.listeners.delete(listener);
      if (!this.listeners.size) clearTimeout(this.timer);
    };
  };

  snapshot = () => this.state;

  private update(change: Partial<TargetConnectionState>) {
    this.state = { ...this.state, ...change };
    for (const listener of this.listeners) listener();
  }

  private accept(status: TargetStatus) {
    const connected = status.phase === "connected";
    this.update({
      status, offline: !connected,
      ...(connected ? { reconnecting: false, asker: null, entry: null, failure: null } : {}),
    });
  }

  private schedule() {
    clearTimeout(this.timer);
    if (!this.listeners.size) return;
    this.timer = setTimeout(() => void this.read(), this.state.reconnecting ? RECONNECTING_MILLISECONDS : IDLE_MILLISECONDS);
  }

  private async read() {
    try {
      this.accept(await this.api.status(this.targetId));
      if (needsPerson(this.state) && !this.state.entry) {
        const entries = await this.api.list();
        this.update({ entry: entries.find(item => item.target.targetId === this.targetId) ?? null });
      }
    } catch (failure) {
      this.update({ offline: true, ...(this.state.reconnecting ? { reconnecting: false, asker: null, failure } : {}) });
    }
    this.schedule();
  }

  reconnect = async (asker: string) => {
    if (this.state.status?.phase === "connecting") return;
    this.update({ failure: null });
    try {
      await this.api.connect(this.targetId, null);
      // Read before watching for a failure, so the last attempt's is not taken for this one's.
      const status = await this.api.status(this.targetId);
      this.update({ reconnecting: true, asker, entry: null });
      this.accept(status);
    } catch (failure) {
      this.update({ failure });
    }
    this.schedule();
  };

  dismiss = () => {
    this.update({ reconnecting: false, asker: null, entry: null });
    this.schedule();
  };
}

export function needsPerson(state: TargetConnectionState): boolean {
  return state.reconnecting && (Boolean(state.status?.prompt) || state.status?.phase === "failed");
}

const connections = new WeakMap<TargetApi, Map<string, TargetConnection>>();

function connectionFor(api: TargetApi, targetId: string): TargetConnection {
  let forApi = connections.get(api);
  if (!forApi) connections.set(api, forApi = new Map());
  let connection = forApi.get(targetId);
  if (!connection) forApi.set(targetId, connection = new TargetConnection(api, targetId));
  return connection;
}

export function useTargetConnection(api: TargetApi, targetId: string) {
  const connection = connectionFor(api, targetId);
  const state = useSyncExternalStore(connection.subscribe, connection.snapshot);
  return { state, reconnect: connection.reconnect, dismiss: connection.dismiss };
}
