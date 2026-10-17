import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { SharedAgentPreferences } from "./sharedAgentPreferences";
import { attachSharedAgentPreferences, readLocalAgentPreferences } from "./agentPreferences";
import { TauriCoreClient } from "./tauriClient";
import { StartupShell } from "./StartupShell";
import { LoadingStatus } from "./LoadingStatus";
import { useLoadingVisibility } from "./useLoadingVisibility";
import "./sharedAgentPreferences.css";

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
  const retry = <button type="button" disabled={state.busy} onClick={() => void store.retry()}>重新核实</button>;
  if (!state.snapshot && state.error) return <StartupShell><div className="startup-recovery"><p role="alert">{state.error}</p>{retry}</div></StartupShell>;
  if (loading.pending) return <StartupShell>{loading.visible ? undefined : <div role="status" aria-label="正在读取 Agent 设置" />}</StartupShell>;
  if (state.importRequired) return <StartupShell><div className="startup-recovery">
    <h1>保留已有 Agent 设置</h1>
    <p>此设备保存过 Agent 的启用状态和排序。将它们保存到后台后，所有窗口将使用同一份设置。</p>
    <p>原有本地副本会保留；如果其他窗口先保存了设置，会采用后台当前值，不覆盖它。</p>
    <div className="startup-actions">
      <button type="button" disabled={state.busy || state.unconfirmed} onClick={() => void store.importLegacy(true)}>保留并同步现有设置</button>
      <button type="button" disabled={state.busy || state.unconfirmed} onClick={() => void store.importLegacy(false)}>使用默认设置</button>
    </div>
    {state.busy && <LoadingStatus>正在保存设置…</LoadingStatus>}
    {state.error && <div role="alert"><p>{state.error}</p>{retry}</div>}
  </div></StartupShell>;
  return <>{children}{state.error && <div className="shared-settings-notice" role="alert"><span>{state.error}</span>{retry}</div>}</>;
}
