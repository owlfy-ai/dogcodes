import { readLocalPreferenceBackup } from "./localPreferenceBackup";
import { editSharedHistoryPreferences, readSharedHistoryPreferences } from "./sharedHistoryPreferences";
import type { HistoryPreferences as SharedHistory } from "@dogcodes/protocol";
export type HistorySectionId = "pinned" | "projects" | "archived";
export type HistorySort = "manual" | "updated";
export interface SectionPreference { collapsed: boolean; sort: HistorySort }
export interface SectionPreferences {
  sections: Record<HistorySectionId, SectionPreference>;
  orders: Record<string, string[]>;
}
export const SECTION_STORAGE_KEY = "dogcodes.history.sections.v1";
export const COLLAPSED_STORAGE_KEY = "dogcodes.history.collapsed.v1";
export function withSharedHistory(local: SectionPreferences, shared: SharedHistory): SectionPreferences {
  const sections = { ...local.sections };
  for (const key of ["pinned", "projects", "archived"] as const) sections[key] = { ...sections[key], sort: shared.sorts[key] ?? "manual" };
  return { sections, orders: shared.orders };
}
export function loadSectionPreferences(): SectionPreferences {
  const local = loadLocalSectionPreferences();
  const shared = readSharedHistoryPreferences();
  return shared ? withSharedHistory(local, shared) : local;
}
export function loadLocalSectionPreferences(): SectionPreferences {
  const result: SectionPreferences = { sections: {
    pinned: { collapsed: false, sort: "manual" },
    projects: { collapsed: false, sort: "manual" },
    archived: { collapsed: true, sort: "manual" },
  }, orders: {} };
  try {
    const raw = JSON.parse(readLocalPreferenceBackup(SECTION_STORAGE_KEY) ?? "null");
    for (const key of ["pinned", "projects", "archived"] as const) {
      const value = raw?.sections?.[key];
      if (typeof value?.collapsed === "boolean") result.sections[key].collapsed = value.collapsed;
      if (value?.sort === "manual" || value?.sort === "updated") result.sections[key].sort = value.sort;
    }
    for (const [scope, ids] of Object.entries(raw?.orders ?? {})) {
      if (Array.isArray(ids) && ids.every(id => typeof id === "string")) result.orders[scope] = [...new Set(ids)] as string[];
    }
  } catch { /* Missing or invalid local preferences use the original order. */ }
  try {
    const collapsed = JSON.parse(localStorage.getItem(COLLAPSED_STORAGE_KEY) ?? "null");
    for (const key of ["pinned", "projects", "archived"] as const) if (typeof collapsed?.[key] === "boolean") result.sections[key].collapsed = collapsed[key];
  } catch { /* Keep legacy layout when the newer local-only copy cannot be read. */ }
  return result;
}
export function saveSectionPreferences(next: SectionPreferences, previous: SectionPreferences): SectionPreferences {
  if (!readSharedHistoryPreferences()) {
    localStorage.setItem(SECTION_STORAGE_KEY, JSON.stringify(next));
    return next;
  }
  const keys = ["pinned", "projects", "archived"] as const;
  if (keys.some(key => next.sections[key].collapsed !== previous.sections[key].collapsed)) {
    // Preserve the original mixed settings as a backup; only layout stays local.
    localStorage.setItem(COLLAPSED_STORAGE_KEY, JSON.stringify(Object.fromEntries(keys.map(key => [key, next.sections[key].collapsed]))));
  }
  const changedSorts = keys.filter(key => next.sections[key].sort !== previous.sections[key].sort);
  const changedOrders = Object.keys(next.orders).filter(key => JSON.stringify(next.orders[key]) !== JSON.stringify(previous.orders[key]));
  if (changedSorts.length || changedOrders.length) editSharedHistoryPreferences(value => {
    const sorts = { ...value.sorts }, orders = { ...value.orders };
    for (const key of changedSorts) sorts[key] = next.sections[key].sort;
    for (const key of changedOrders) orders[key] = [...next.orders[key]];
    return { ...value, sorts, orders };
  });
  return loadSectionPreferences();
}
export function historyTime(value?: string | null): number {
  const time = value ? Date.parse(value) : 0;
  return Number.isFinite(time) ? time : 0;
}
export function orderHistory<T>(items: T[], mode: HistorySort, order: string[] | undefined, id: (item: T) => string, time: (item: T) => number): T[] {
  const ranks = new Map(order?.map((key, index) => [key, index]));
  return [...items].sort((a, b) => mode === "updated"
    ? time(b) - time(a)
    : (ranks.get(id(a)) ?? Number.MAX_SAFE_INTEGER) - (ranks.get(id(b)) ?? Number.MAX_SAFE_INTEGER));
}
export function moveHistoryItem(ids: string[], source: string, target: string, edge?: "before" | "after"): string[] {
  if (source === target || !ids.includes(source) || !ids.includes(target)) return ids;
  const next = ids.filter(id => id !== source);
  const index = edge ? next.indexOf(target) + Number(edge === "after") : ids.indexOf(target);
  next.splice(index, 0, source);
  return next;
}
