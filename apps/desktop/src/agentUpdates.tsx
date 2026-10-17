import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { focusDialogTitle } from "./dialogFocus";
import type { AgentUpdate, BootstrapSnapshot, CoreClient, InstallationSummary } from "@dogcodes/protocol";
import "./agentUpdates.css";
import { compareVersions } from "./agentOptions";

const pending = (id: string): AgentUpdate => ({ installationId: id, status: "checking", currentVersion: null, latestVersion: null, source: "", message: null, canUpgrade: false, checkedAt: 0 });
export function useAgentUpdates(client: CoreClient, installations: InstallationSummary[], refresh: (snapshot: BootstrapSnapshot) => void) {
  const [updates, setUpdates] = useState<Record<string, AgentUpdate>>({});
  const busy = useRef(new Set<string>());
  const confirmations = useRef(new Set<string>());
  const externalUpdates = useRef(new Map<string, string>());
  const refreshRef = useRef(refresh); refreshRef.current = refresh;
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const accept = useCallback((value: AgentUpdate) => { if (alive.current) setUpdates(old => ({ ...old, [value.installationId]: value })); }, []);
  const check = useCallback(async (id: string, force = false) => {
    if (!client.checkAgentUpdate || busy.current.has(id) || confirmations.current.has(id)) return;
    busy.current.add(id);
    if (alive.current) setUpdates(old => ({ ...old, [id]: { ...pending(id), desktopVersion: old[id]?.desktopVersion } }));
    try { accept(await client.checkAgentUpdate(id, force)); }
    catch { accept({ ...pending(id), status: "error", message: "版本检查失败，请稍后重试" }); }
    finally { busy.current.delete(id); }
  }, [client, accept]);
  useEffect(() => {
    const onFocus = () => {
      for (const [id, provider] of externalUpdates.current) {
        externalUpdates.current.delete(id);
        void client.scanAgent(provider).then(async snapshot => {
          if (!alive.current) return;
          refreshRef.current(snapshot);
          const installation = snapshot.installations.find(i => i.providerId === provider);
          if (installation) await check(installation.installationId, true);
        }).catch(() => { void check(id, true); });
      }
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [client, check]);
  const installationKey = installations.filter(i => i.providerId !== "fake").map(i => `${i.installationId}:${i.fingerprint}:${i.version}`).sort().join("|");
  useEffect(() => {
    // Start at app bootstrap, not on the first visit to the quota page. The native cache
    // owns TTL / concurrent checks; install fingerprints invalidate previous results.
    for (const i of installations) if (i.providerId !== "fake") void check(i.installationId);
    const timer = window.setInterval(() => { for (const i of installations) if (i.providerId !== "fake") void check(i.installationId); }, 6 * 60 * 60 * 1000);
    return () => window.clearInterval(timer);
  }, [installationKey, check]);
  useEffect(() => {
    let disposed = false; let unlisten: (() => void) | undefined;
    void client.onAgentUpdate?.(value => {
      accept(value);
      if (value.status === "updated") void client.bootstrap().then(s => { if (!disposed) refreshRef.current(s); }).catch(() => {});
    }).then(fn => { if (disposed) fn(); else unlisten = fn; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, [client, accept]);
  const upgrade = useCallback(async (id: string, confirmationToken?: string) => {
    if (!client.upgradeAgent || busy.current.has(id)) return;
    busy.current.add(id);
    setUpdates(old => ({ ...old, [id]: { ...(old[id] ?? pending(id)), status: "upgrading", canUpgrade: false, message: "正在检查升级条件" } }));
    try {
      const value = await client.upgradeAgent(id, confirmationToken);
      if (value.status === "in_use") {
        confirmations.current.add(id);
        if (alive.current) setUpdates(old => ({ ...old, [id]: { ...value, latestVersion: old[id]?.latestVersion ?? value.latestVersion, source: old[id]?.source ?? value.source } }));
        return;
      }
      confirmations.current.delete(id); accept(value);
      // Refresh even after a partial updater failure: installed binaries may have changed.
      const snapshot = await client.bootstrap(); if (alive.current) refreshRef.current(snapshot);
    } catch (e) {
      confirmations.current.delete(id);
      if (alive.current) setUpdates(old => ({ ...old, [id]: { ...(old[id] ?? pending(id)), status: "error", canUpgrade: Boolean(old[id]?.latestVersion), message: typeof e === "string" ? e : "升级失败，请稍后重试" } }));
    } finally { busy.current.delete(id); }
  }, [client, accept]);
  const cancelUpgrade = useCallback((id: string) => {
    confirmations.current.delete(id);
    setUpdates(old => ({ ...old, [id]: { ...old[id], status: "available", canUpgrade: true, inUse: undefined, message: null } }));
  }, []);
  const openUpdater = useCallback(async (id: string) => {
    if (!client.openAgentUpdater) return;
    const installation = installations.find(i => i.installationId === id);
    if (installation) externalUpdates.current.set(id, installation.providerId);
    try { await client.openAgentUpdater(id); }
    catch {
      externalUpdates.current.delete(id);
      setUpdates(old => ({ ...old, [id]: { ...(old[id] ?? pending(id)), status: "error", message: "无法打开宿主应用，请手动打开并检查更新" } }));
    }
  }, [client, installations]);
  return { updates, check, upgrade, openUpdater, cancelUpgrade };
}
export type AgentUpdateActions = { updates: Record<string, AgentUpdate>; check: (id: string, force?: boolean) => void; upgrade: (id: string, confirmationToken?: string) => void; openUpdater?: (id: string) => void; cancelUpgrade?: (id: string) => void };

function UpgradeConfirmation({ update, actions }: { update: AgentUpdate; actions: AgentUpdateActions }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    dialog.current?.showModal(); focusDialogTitle(dialog.current);
    return () => { dialog.current?.close(); if (previous?.isConnected) previous.focus(); };
  }, []);
  const usage = update.inUse!;
  return createPortal(<dialog ref={dialog} className="agent-upgrade-dialog" aria-labelledby="agent-upgrade-title"
    onCancel={event => { event.preventDefault(); actions.cancelUpgrade?.(update.installationId); }}>
    <h2 id="agent-upgrade-title" tabIndex={-1} data-dialog-title>Agent 正在使用中</h2>
    <p>检测到此安装有{usage.taskCount > 0 ? ` ${usage.taskCount} 个本软件内任务，` : ""} {usage.processCount} 个相关进程正在运行。</p>
    <p>结束后将中断当前操作，包括其他终端中的相关任务。已修改的文件会保留，未完成的工作可能需要重新执行。</p>
    <footer><button type="button" onClick={() => actions.cancelUpgrade?.(update.installationId)}>取消升级</button>
      <button className="agent-upgrade-confirm" type="button" onClick={() => actions.upgrade(update.installationId, usage.token)}>结束任务并升级</button></footer>
  </dialog>, document.body);
}
function UpdateIcon({ kind }: { kind: "upgrade" | "retry" | "external" | "done" | "info" }) {
  const paths = {
    upgrade: "M12 16V4m-4 4 4-4 4 4M5 14v5h14v-5",
    retry: "M20 7v5h-5M4 17v-5h5M6.1 7a7 7 0 0 1 11.6-1L20 9M4 15l2.3 3A7 7 0 0 0 17.9 17",
    external: "M14 4h6v6m0-6-9 9M10 5H5v14h14v-5",
    done: "m5 12 4 4L19 6",
    info: "M12 11v5m0-9v.01M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0",
  };
  return <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[kind]} /></svg>;
}
function UpgradeProgress({ message }: { message: string }) {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    const started = Date.now();
    const timer = window.setInterval(() => setSeconds(Math.floor((Date.now() - started) / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, []);
  const elapsed = seconds < 60 ? `${seconds} 秒` : `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`;
  return <span className="agent-update-progress" role="status" aria-label={`正在升级：${message}`}>
    <i className="agent-update-spinner" aria-hidden="true" />
    <span>{message}<span aria-hidden="true"> · {elapsed}</span></span>
  </span>;
}
export function AgentUpdateControl({ id, actions, detailed = false }: { id: string; actions?: AgentUpdateActions; detailed?: boolean }) {
  const update = actions?.updates[id];
  if (!update || (!detailed && update.status === "up_to_date")) return null;
  if (update.status === "in_use" && update.inUse && actions) return <UpgradeConfirmation update={update} actions={actions} />;
  const hostUpdate = update.status === "managed" && update.latestVersion && update.currentVersion && compareVersions(update.latestVersion, update.currentVersion) > 0;
  if (!detailed && update.status === "managed" && !hostUpdate) return null;
  const working = update.status === "checking" || update.status === "upgrading";
  const retryUpgrade = update.status === "error" && update.canUpgrade && Boolean(update.latestVersion);
  const label = update.status === "up_to_date" ? "已是最新版本" : update.status === "checking" ? "检查版本中…" : update.status === "upgrading" ? "正在升级…" : update.status === "available" ? `升级至 ${update.latestVersion}` : update.status === "updated" ? "升级完成" : update.status === "managed" ? update.message ?? "在宿主应用中检查更新" : update.message ?? "版本检查失败";
  const hint = [label, update.source, update.checkedAt ? `检查于 ${new Date(update.checkedAt).toLocaleString()}` : ""].filter(Boolean).join(" · ");
  return <span className={`agent-update agent-update--${update.status}`} aria-live="polite" aria-busy={working} data-tooltip={hint}>
    {detailed && ["up_to_date", "managed", "unsupported", "error", "updated"].includes(update.status) && <span>{label}</span>}
    {(update.status === "available" || hostUpdate) && <span className="agent-update-version" data-tooltip={`${hostUpdate ? "桌面应用有新版本" : "有新版本"} ${update.latestVersion}`} data-tooltip-overflow="">{hostUpdate ? "桌面应用有新版本" : "有新版本"} {update.latestVersion}</span>}
    {update.status === "checking" && <span className="agent-update-status" role="status" aria-label={label}><i className="agent-update-spinner" aria-hidden="true" /></span>}
    {update.status === "upgrading" && <UpgradeProgress message={update.message ?? "正在下载并安装，较大的更新可能需要几分钟"} />}
    {update.status === "available" && update.canUpgrade && <button className="agent-update-action" type="button" data-tooltip={hint} aria-label={label} onClick={() => actions?.upgrade(id)}><UpdateIcon kind="upgrade" />{detailed && "升级"}</button>}
    {update.status === "managed" && actions?.openUpdater && <button className="agent-update-action" type="button" data-tooltip={hint} aria-label="打开宿主应用检查更新" onClick={() => actions.openUpdater?.(id)}><UpdateIcon kind="external" />{detailed && "打开应用"}</button>}
    {update.status === "error" && <button className="agent-update-action" type="button" data-tooltip={`${hint} · 点击${retryUpgrade ? "重试升级" : "重新检查"}`} aria-label={`${retryUpgrade ? "重试升级" : "重新检查版本"}：${label}`} onClick={() => retryUpgrade ? actions?.upgrade(id) : actions?.check(id, true)}><UpdateIcon kind="retry" /></button>}
    {(update.status === "updated" || update.status === "unsupported") && <span className="agent-update-status" role="status" aria-label={label}><UpdateIcon kind={update.status === "updated" ? "done" : "info"} /></span>}
  </span>;
}
