import { useSyncExternalStore } from "react";
import type { CoreClient, HistoryPreferences } from "@dogcodes/protocol";
import { SharedPreferences } from "./sharedPreferences";
import { translate as t } from "./i18n";

type Client = Required<Pick<CoreClient, "historyPreferences" | "setHistoryPreferences" | "subscribe">>;
export const emptyHistoryPreferences = (): HistoryPreferences => ({ tasks: {}, projects: {}, sorts: {}, orders: {} });
export class SharedHistoryPreferences extends SharedPreferences<HistoryPreferences> {
  constructor(client: Client, legacy: HistoryPreferences) {
    super({ read: () => client.historyPreferences(), write: (value, revision, id) => client.setHistoryPreferences(value, revision, id), subscribe: (after, listener) => client.subscribe(after, listener) }, legacy, {
      streamId: "preferences.history", eventType: "preferences.history.updated", label: "sharedPrefs.historyLabel", defaults: emptyHistoryPreferences,
      hasLegacy: value => Object.keys(value.tasks).length > 0 || Object.keys(value.projects).length > 0
        || Object.values(value.sorts).some(sort => sort !== "manual") || Object.values(value.orders).some(order => order.length > 0),
      valid: validHistoryPreferences,
    });
  }
}
function validHistoryPreferences(value: HistoryPreferences) {
  const keyValid = (key: string) => key.length > 0 && new TextEncoder().encode(key).length <= 512 && !/[\u0000-\u001f\u007f-\u009f]/u.test(key);
  return [value.tasks, value.projects, value.orders].every(map => Object.keys(map).length <= 20_000 && Object.keys(map).every(keyValid))
    && Object.values(value.tasks).every(state => state === "pinned" || state === "archived")
    && Object.values(value.projects).every(state => state === "pinned" || state === "removed")
    && Object.entries(value.sorts).every(([key, sort]) => ["pinned", "projects", "archived"].includes(key) && (sort === "manual" || sort === "updated"))
    && Object.values(value.orders).reduce((total, ids) => total + ids.length, 0) <= 100_000
    && Object.values(value.orders).every(ids => ids.every(keyValid) && new Set(ids).size === ids.length)
    && new TextEncoder().encode(JSON.stringify(value)).length <= 4 * 1024 * 1024;
}

const changedEvent = "dogcodes:history-preferences";
let shared: SharedHistoryPreferences | null = null;
export function attachSharedHistoryPreferences(store: SharedHistoryPreferences) {
  shared = store;
  const notify = () => window.dispatchEvent(new Event(changedEvent));
  const stop = store.subscribe(notify); notify();
  return () => { stop(); if (shared === store) shared = null; notify(); };
}
export const retrySharedHistoryPreferences = () => shared?.retry();
export const readSharedHistoryPreferences = () => shared?.getSnapshot().snapshot?.preferences ?? null;
const readView = () => shared?.getSnapshot() ?? null;
function subscribe(listener: () => void) {
  window.addEventListener(changedEvent, listener);
  return () => window.removeEventListener(changedEvent, listener);
}
export const useSharedHistoryPreferences = () => useSyncExternalStore(subscribe, readView, () => null);

/** Queue an explicit edit against the current host revision. No automatic rebase on conflict. */
export function editSharedHistoryPreferences(edit: (value: HistoryPreferences) => HistoryPreferences): boolean {
  if (!shared) return false;
  const state = shared.getSnapshot();
  if (!state.snapshot || state.busy || state.unconfirmed || state.importRequired) throw new Error(t("sharedPrefs.verifyHistory"));
  void shared.write(edit(state.snapshot.preferences));
  return true;
}
