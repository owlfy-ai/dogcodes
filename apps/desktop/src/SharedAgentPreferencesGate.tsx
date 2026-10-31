import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { SharedAgentPreferences } from "./sharedAgentPreferences";
import { attachSharedAgentPreferences, readLocalAgentPreferences } from "./agentPreferences";
import { TauriCoreClient } from "./tauriClient";
import { StartupShell } from "./StartupShell";
import { LoadingStatus } from "./LoadingStatus";
import { useLoadingVisibility } from "./useLoadingVisibility";
import "./sharedAgentPreferences.css";
import { translate as t } from "./i18n";

export function SharedAgentPreferencesGate({ children }: { children: ReactNode }) {
  return isTauri() ? <ConnectedSettings>{children}</ConnectedSettings> : children;
}

function ConnectedSettings({ children }: { children: ReactNode }) {
  const [store] = useState(() => new SharedAgentPreferences(new TauriCoreClient(), readLocalAgentPreferences()));
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  const loading = useLoadingVisibility(!state.snapshot && !state.error);
  useEffect(() => {
    const detach = attachSharedAgentPreferences(store);
    store.start();
    return () => { store.stop(); detach(); };
  }, [store]);
  const retry = <button type="button" disabled={state.busy} onClick={() => void store.retry()}>{t("sharedPrefs.retry")}</button>;
  if (!state.snapshot && state.error) return <StartupShell><div className="startup-recovery"><p role="alert">{state.error}</p>{retry}</div></StartupShell>;
  if (loading.pending) return <StartupShell>{loading.visible ? undefined : <div role="status" aria-label={t("sharedPrefs.loadingAgents")} />}</StartupShell>;
  if (state.importRequired) return <StartupShell><div className="startup-recovery">
    <h1>{t("sharedPrefs.agentsTitle")}</h1>
    <p>{t("sharedPrefs.agentsDescription")}</p>
    <p>{t("sharedPrefs.localCopy")}</p>
    <div className="startup-actions">
      <button type="button" disabled={state.busy || state.unconfirmed} onClick={() => void store.importLegacy(true)}>{t("sharedPrefs.keep")}</button>
      <button type="button" disabled={state.busy || state.unconfirmed} onClick={() => void store.importLegacy(false)}>{t("sharedPrefs.useDefault")}</button>
    </div>
    {state.busy && <LoadingStatus>{t("sharedPrefs.saving")}</LoadingStatus>}
    {state.error && <div role="alert"><p>{state.error}</p>{retry}</div>}
  </div></StartupShell>;
  return <>{children}{state.error && <div className="shared-settings-notice" role="alert"><span>{state.error}</span>{retry}</div>}</>;
}
