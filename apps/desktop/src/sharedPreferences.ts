import type { CoreClient, EventStreamMessage } from "@dogcodes/protocol";
import { ulid } from "ulid";
import { translate as t } from "./i18n";

export interface PreferencesSnapshot<T> { revision: number; lastSequence: number; preferences: T }
export interface PreferencesClient<T> {
  read(): Promise<PreferencesSnapshot<T>>;
  write(preferences: T, revision: number, operationId: string): Promise<void>;
  subscribe: CoreClient["subscribe"];
}
export interface PreferencesOptions<T> {
  // label is a translation key naming the settings in messages.
  streamId: string; eventType: string; label: string;
  defaults(): T; hasLegacy(value: T): boolean; valid(value: T): boolean;
}
type Pending<T> = { preferences: T; revision: number; operationId: string };
export interface SharedPreferencesView<T> {
  snapshot: PreferencesSnapshot<T> | null;
  importRequired: boolean;
  busy: boolean;
  unconfirmed: boolean;
  error: string;
}

/** One window's cache of host-owned settings. Only a durable ack or snapshot changes the UI. */
export class SharedPreferences<T> {
  private view: SharedPreferencesView<T> = { snapshot: null, importRequired: false, busy: false, unconfirmed: false, error: "" };
  private listeners = new Set<() => void>();
  private stopStream: (() => void) | undefined;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private generation = 0;
  private active = false;
  private pending: Pending<T> | null = null;
  private cursor = 0;
  private retryDelay = 500;
  constructor(private client: PreferencesClient<T>, private legacy: T, private options: PreferencesOptions<T>) {}
  getSnapshot = () => this.view;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(patch: Partial<SharedPreferencesView<T>>) {
    this.view = { ...this.view, ...patch };
    for (const listener of this.listeners) listener();
  }
  start() { this.active = true; void this.connect(); }
  stop() {
    this.active = false; this.generation++;
    this.stopStream?.(); this.stopStream = undefined;
    clearTimeout(this.timer);
  }
  private accept(snapshot: PreferencesSnapshot<T>) {
    if (this.view.snapshot && snapshot.revision < this.view.snapshot.revision) return;
    this.update({ snapshot, importRequired: snapshot.revision === 0 && this.options.hasLegacy(this.legacy) });
  }
  private async read() {
    return this.client.read();
  }
  private async connect() {
    const generation = ++this.generation;
    clearTimeout(this.timer);
    this.stopStream?.(); this.stopStream = undefined;
    try {
      const snapshot = await this.read();
      if (!this.active || generation !== this.generation) return;
      this.accept(snapshot);
      this.cursor = snapshot.lastSequence;
      const stop = await this.client.subscribe(snapshot.lastSequence, message => {
        if (this.active && generation === this.generation) this.onMessage(message);
      });
      if (!this.active || generation !== this.generation) { stop(); return; }
      this.stopStream = stop;
      this.retryDelay = 500;
      if (!this.pending) this.update({ error: "" });
    } catch {
      if (this.active && generation === this.generation) this.reconnect();
    }
  }
  private reconnect() {
    this.generation++;
    this.stopStream?.(); this.stopStream = undefined;
    clearTimeout(this.timer);
    this.update({ error: t("sharedPrefs.syncLost", { label: t(this.options.label) }) });
    this.timer = setTimeout(() => { void this.connect(); }, this.retryDelay);
    this.retryDelay = Math.min(this.retryDelay * 2, 8_000);
  }
  private onMessage(message: EventStreamMessage) {
    if (message.type === "error") { this.reconnect(); return; }
    if (message.type !== "event") return;
    const event = message.data;
    if (event.sequence <= this.cursor) return;
    if (event.sequence !== this.cursor + 1) { this.reconnect(); return; }
    this.cursor = event.sequence;
    if (event.eventType === this.options.eventType && event.stream.streamType === "system" && event.stream.streamId === this.options.streamId) {
      this.accept({ revision: event.streamVersion, lastSequence: event.sequence, preferences: event.payload as unknown as T });
    }
  }
  async importLegacy(useLegacy: boolean) {
    if (!this.view.importRequired) return;
    await this.write(useLegacy ? this.legacy : this.options.defaults(), true);
  }
  async write(preferences: T, importing = false) {
    if (!this.active || !this.view.snapshot || this.view.busy || this.pending || (this.view.importRequired && !importing)) return;
    // Old local storage may be corrupt or edited externally. Reject before
    // issuing a command, so a known-invalid import never becomes "unconfirmed"
    // and prevents the user from choosing defaults. Keep the original backup.
    if (!this.options.valid(preferences)) {
      this.update({ error: importing
        ? t("sharedPrefs.invalidImport", { label: t(this.options.label) })
        : t("sharedPrefs.invalid", { label: t(this.options.label) }) });
      return;
    }
    this.pending = {
      preferences: structuredClone(preferences),
      revision: this.view.snapshot.revision, operationId: `cmd_${ulid()}`,
    };
    await this.sendPending();
  }
  async retry() {
    if (this.view.busy) return;
    if (this.pending) await this.sendPending();
    else await this.connect();
  }
  private async sendPending() {
    const pending = this.pending;
    if (!pending) return;
    this.update({ busy: true, unconfirmed: true, error: "" });
    try {
      await this.client.write(pending.preferences, pending.revision, pending.operationId);
      // An accepted command is committed before its ack. A newer subscription
      // update wins over this request's version, including another client's edit.
      if (this.active) this.accept({ revision: pending.revision + 1, lastSequence: this.cursor, preferences: pending.preferences });
      this.pending = null;
    } catch {
      try {
        const current = await this.read();
        if (this.active) this.accept(current);
        if (current.revision > pending.revision) {
          // Whether our write or another client's write won, the old CAS can no
          // longer overwrite current state. Never silently resend with a new ID.
          this.pending = null;
          if (this.active) this.update({ error: t("sharedPrefs.saveNotConfirmed") });
        } else if (this.active) {
          this.update({ error: t("sharedPrefs.saveUnverified") });
        }
      } catch {
        if (this.active) this.update({ error: t("sharedPrefs.saveUnknown") });
      }
    } finally {
      this.update({ busy: false, unconfirmed: this.pending !== null });
    }
  }
}
