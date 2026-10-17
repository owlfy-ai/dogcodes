import { readLocalPreferenceBackup } from "./localPreferenceBackup";
import { editSharedHistoryPreferences, readSharedHistoryPreferences } from "./sharedHistoryPreferences";
import type { TaskId } from "@dogcodes/protocol";

const STORAGE_KEY = "dogcodes.history.preferences.v1";
export type HistoryPreferences = Record<string, "pinned" | "archived">;

export function loadHistoryPreferences(): HistoryPreferences {
  return readSharedHistoryPreferences()?.tasks ?? loadLocalHistoryPreferences();
}
export function loadLocalHistoryPreferences(): HistoryPreferences {
  const raw = readLocalPreferenceBackup(STORAGE_KEY);
  if (!raw) return {};
  const value: unknown = JSON.parse(raw);
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(Object.entries(value).filter(([, status]) => status === "pinned" || status === "archived")) as HistoryPreferences;
}

export function saveHistoryPreference(taskId: TaskId, status: "pinned" | "archived" | null): HistoryPreferences {
  if (editSharedHistoryPreferences(value => {
    const tasks = { ...value.tasks };
    if (status) tasks[taskId] = status; else delete tasks[taskId];
    return { ...value, tasks };
  })) return loadHistoryPreferences();
  const next = loadHistoryPreferences();
  if (status) next[taskId] = status;
  else delete next[taskId];
  // Commit before changing the UI so a failed save never looks successful.
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  return next;
}
