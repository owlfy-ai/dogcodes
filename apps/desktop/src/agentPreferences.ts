import { readLocalPreferenceBackup } from "./localPreferenceBackup";
import { useSyncExternalStore } from "react";
import type { SharedAgentPreferences, SharedAgentPreferencesView } from "./sharedAgentPreferences";

export const AGENT_PREFERENCES_KEY = "dogcodes.agents.preferences.v1";
const changedEvent = "dogcodes:agent-preferences";
export interface AgentPreferences { order: string[]; disabled: string[] }
const defaults: AgentPreferences = { order: [], disabled: [] };
// A common fallback order also covers menus whose native discovery order differs.
const defaultOrder = ["codex", "claude-code", "grok", "cursor", "antigravity", "zcode", "kimi", "codebuddy", "pi", "opencode", "deepseek-harness", "kiro"];
let cachedRaw: string | null | undefined;
let cached = defaults;
let memoryOnly = false;
let shared: SharedAgentPreferences | null = null;
const localStatus: SharedAgentPreferencesView = { snapshot: null, importRequired: false, busy: false, unconfirmed: false, error: "" };
export function attachSharedAgentPreferences(store: SharedAgentPreferences) {
  shared = store;
  const notify = () => window.dispatchEvent(new Event(changedEvent));
  const stop = store.subscribe(notify);
  notify();
  return () => { stop(); if (shared === store) shared = null; notify(); };
}
const strings = (value: unknown): string[] => Array.isArray(value) ? [...new Set(value.filter((item): item is string => typeof item === "string" && item.length > 0))] : [];
export function readAgentPreferences(): AgentPreferences {
  if (shared) return shared.getSnapshot().snapshot?.preferences ?? defaults;
  return readLocalAgentPreferences();
}
export function readLocalAgentPreferences(): AgentPreferences {
  if (memoryOnly) return cached;
  let raw: string | null;
  try { raw = readLocalPreferenceBackup(AGENT_PREFERENCES_KEY); } catch { return cached; }
  if (raw === cachedRaw) return cached;
  cachedRaw = raw;
  try { const value = JSON.parse(raw ?? "null"); cached = value ? { order: strings(value.order), disabled: strings(value.disabled) } : defaults; }
  catch { cached = defaults; }
  return cached;
}
function save(preferences: AgentPreferences) {
  if (shared) { void shared.write(preferences); return; }
  cached = preferences;
  cachedRaw = JSON.stringify(preferences);
  try { localStorage.setItem(AGENT_PREFERENCES_KEY, cachedRaw); memoryOnly = false; } catch { memoryOnly = true; }
  window.dispatchEvent(new Event(changedEvent));
}
function subscribe(listener: () => void) {
  const onStorage = (event: StorageEvent) => { if (event.key === AGENT_PREFERENCES_KEY || event.key === null) { memoryOnly = false; listener(); } };
  window.addEventListener(changedEvent, listener);
  window.addEventListener("storage", onStorage);
  return () => { window.removeEventListener(changedEvent, listener); window.removeEventListener("storage", onStorage); };
}
export const useAgentPreferences = () => useSyncExternalStore(subscribe, readAgentPreferences, () => defaults);
const readWriteState = () => shared?.getSnapshot() ?? localStatus;
export const useAgentPreferencesWriteState = () => useSyncExternalStore(subscribe, readWriteState, () => localStatus);
export function orderAgents<T>(items: readonly T[], provider: (item: T) => string, preferences: AgentPreferences): T[] {
  const order = [...new Set([...preferences.order, ...defaultOrder])];
  const rank = (id: string) => { const index = order.indexOf(id); return index < 0 ? order.length : index; };
  return [...items].sort((a, b) => rank(provider(a)) - rank(provider(b)) || (rank(provider(a)) === order.length ? provider(a).localeCompare(provider(b)) : 0));
}
/** Input is the caller's installed/ready list. Preferences never manufacture executors. */
export function enabledAgents<T>(items: readonly T[], provider: (item: T) => string, preferences: AgentPreferences): T[] {
  return orderAgents(items.filter(item => provider(item) !== "fake" && !preferences.disabled.includes(provider(item))), provider, preferences);
}
export function setAgentEnabled(providerId: string, enabled: boolean) {
  const preferences = readAgentPreferences();
  save({ ...preferences, disabled: enabled ? preferences.disabled.filter(id => id !== providerId) : [...new Set([...preferences.disabled, providerId])] });
}
export function moveAgent(providerId: string, targetId: string, visibleOrder: string[]) {
  const order = [...new Set(visibleOrder)];
  const from = order.indexOf(providerId), to = order.indexOf(targetId);
  if (from < 0 || to < 0 || from === to) return;
  order.splice(from, 1); order.splice(to, 0, providerId);
  const preferences = readAgentPreferences();
  save({ ...preferences, order: [...order, ...preferences.order.filter(id => !order.includes(id))] });
}
export function useEnabledAgents<T extends { providerId: string }>(items: readonly T[]) {
  return enabledAgents(items, item => item.providerId, useAgentPreferences());
}
