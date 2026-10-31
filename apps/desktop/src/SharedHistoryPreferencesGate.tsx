import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { SharedHistoryPreferences } from "./sharedHistoryPreferences";
import { attachSharedHistoryPreferences } from "./sharedHistoryPreferences";
import { loadLocalHistoryPreferences } from "./historyPreferences";
import { loadLocalProjectHistoryPreferences } from "./projectHistoryPreferences";
import { loadLocalSectionPreferences } from "./historySections";
import type { HistoryPreferences } from "@dogcodes/protocol";

import { TauriCoreClient } from "./tauriClient";
import { StartupShell } from "./StartupShell";
import { LoadingStatus } from "./LoadingStatus";
import { useLoadingVisibility } from "./useLoadingVisibility";
import "./sharedAgentPreferences.css";
import { translate as t } from "./i18n";

export function readLegacyHistoryPreferences(): HistoryPreferences {
  const sections = loadLocalSectionPreferences();
  let tasks: HistoryPreferences["tasks"] = {};
  try { tasks = loadLocalHistoryPreferences(); } catch { /* Leave an unreadable local backup untouched. */ }
  return { tasks, projects: loadLocalProjectHistoryPreferences(), orders: sections.orders,
    sorts: Object.fromEntries(Object.entries(sections.sections).map(([key, value]) => [key, value.sort])) };
}
export function SharedHistoryPreferencesGate({ children }: { children: ReactNode }) {
  return isTauri() ? <ConnectedSettings>{children}</ConnectedSettings> : children;
}

function ConnectedSettings({ children }: { children: ReactNode }) {
  const [store] = useState(() => new SharedHistoryPreferences(new TauriCoreClient(), readLegacyHistoryPreferences()));
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  const loading = useLoadingVisibility(!state.snapshot && !state.error);
  useEffect(() => {
    const detach = attachSharedHistoryPreferences(store);
    store.start();
    return () => { store.stop(); detach(); };
  }, [store]);
  const retry = <button type="button" disabled={state.busy} onClick={() => void store.retry()}>{t("sharedPrefs.retry")}</button>;
  if (!state.snapshot && state.error) return <StartupShell><div className="startup-recovery"><p role="alert">{state.error}</p>{retry}</div></StartupShell>;
  if (loading.pending) return <StartupShell>{loading.visible ? undefined : <div role="status" aria-label={t("sharedPrefs.loadingHistory")} />}</StartupShell>;
  if (state.importRequired) return <StartupShell><div className="startup-recovery">
    <h1>{t("sharedPrefs.historyTitle")}</h1>
    <p>{t("sharedPrefs.historyDescription")}</p>
    <p>{t("sharedPrefs.localCopy")}</p>
    <div className="startup-actions">
      <button type="button" disabled={state.busy || state.unconfirmed} onClick={() => void store.importLegacy(true)}>{t("sharedPrefs.keep")}</button>
      <button type="button" disabled={state.busy || state.unconfirmed} onClick={() => void store.importLegacy(false)}>{t("sharedPrefs.useDefault")}</button>
    </div>
    {state.busy && <LoadingStatus>{t("sharedPrefs.saving")}</LoadingStatus>}
    {state.error && <div role="alert"><p>{state.error}</p>{retry}</div>}
  </div></StartupShell>;
  return children;
}
