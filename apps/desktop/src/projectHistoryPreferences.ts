import { readLocalPreferenceBackup } from "./localPreferenceBackup";
import { editSharedHistoryPreferences, readSharedHistoryPreferences } from "./sharedHistoryPreferences";
export const PROJECT_HISTORY_KEY = "dogcodes.history.projects.v1";
export type ProjectHistoryPreferences = Record<string, "pinned" | "removed">;
export function loadProjectHistoryPreferences(): ProjectHistoryPreferences {
  return readSharedHistoryPreferences()?.projects ?? loadLocalProjectHistoryPreferences();
}
export function loadLocalProjectHistoryPreferences(): ProjectHistoryPreferences {
  try {
    const value: unknown = JSON.parse(readLocalPreferenceBackup(PROJECT_HISTORY_KEY) ?? "{}");
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    return Object.fromEntries(Object.entries(value).filter(([, state]) => state === "pinned" || state === "removed")) as ProjectHistoryPreferences;
  } catch { return {}; }
}
export function saveProjectHistoryPreference(id: string, state: "pinned" | "removed" | null): ProjectHistoryPreferences {
  if (editSharedHistoryPreferences(value => {
    const projects = { ...value.projects };
    if (state) projects[id] = state; else delete projects[id];
    return { ...value, projects };
  })) return loadProjectHistoryPreferences();
  const next = loadProjectHistoryPreferences();
  if (state) next[id] = state;
  else delete next[id];
  localStorage.setItem(PROJECT_HISTORY_KEY, JSON.stringify(next));
  return next;
}
