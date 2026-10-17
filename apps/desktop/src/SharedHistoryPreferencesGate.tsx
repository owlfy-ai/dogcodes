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
  const retry = <button type="button" disabled={state.busy} onClick={() => void store.retry()}>重新核实</button>;
  if (!state.snapshot && state.error) return <StartupShell><div className="startup-recovery"><p role="alert">{state.error}</p>{retry}</div></StartupShell>;
  if (loading.pending) return <StartupShell>{loading.visible ? undefined : <div role="status" aria-label="正在读取历史列表设置" />}</StartupShell>;
  if (state.importRequired) return <StartupShell><div className="startup-recovery">
    <h1>保留已有历史列表设置</h1>
    <p>此设备保存过会话置顶、归档或列表排序。保存到后台后，所有窗口将使用同一份列表设置；折叠状态仍留在当前端。</p>
    <p>原有本地副本会保留；如果其他窗口先保存了设置，会采用后台当前值，不覆盖它。</p>
    <div className="startup-actions">
      <button type="button" disabled={state.busy || state.unconfirmed} onClick={() => void store.importLegacy(true)}>保留并同步现有设置</button>
      <button type="button" disabled={state.busy || state.unconfirmed} onClick={() => void store.importLegacy(false)}>使用默认设置</button>
    </div>
    {state.busy && <LoadingStatus>正在保存设置…</LoadingStatus>}
    {state.error && <div role="alert"><p>{state.error}</p>{retry}</div>}
  </div></StartupShell>;
  return children;
}
