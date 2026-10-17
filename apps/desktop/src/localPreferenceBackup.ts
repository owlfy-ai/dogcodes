// Read-only import sources from before the product rename. An explicitly saved
// current value (including an empty object) always wins. Never delete old copies.
const previousKeys: Record<string, string> = {
  "dogcodes.agents.preferences.v1": "codesheepdog.agents.preferences.v1",
  "dogcodes.history.preferences.v1": "codesheepdog.history.preferences.v1",
  "dogcodes.history.projects.v1": "codesheepdog.history.projects.v1",
  "dogcodes.history.sections.v1": "codesheepdog.history.sections.v1",
};
export function readLocalPreferenceBackup(key: string): string | null {
  const current = localStorage.getItem(key);
  return current ?? (previousKeys[key] ? localStorage.getItem(previousKeys[key]) : null);
}
