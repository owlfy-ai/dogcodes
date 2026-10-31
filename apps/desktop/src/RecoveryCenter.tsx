import { LoadingStatus } from "./LoadingStatus";
import { useCallback, useEffect, useState } from "react";
import type { CoreClient, RecoveryCode, RecoveryItem, RecoverySnapshot, TaskId } from "@dogcodes/protocol";
import { translate as t } from "./i18n";

// Title and hint translation keys per recovery code.
export const recoveryMessages: Record<RecoveryCode, [string, string]> = {
  installation_unconfigured: ["recovery.code.unconfigured", "recovery.code.unconfiguredHint"],
  installation_broken: ["recovery.code.broken", "recovery.code.brokenHint"],
  installation_unsupported: ["recovery.code.unsupported", "recovery.code.unsupportedHint"],
  installation_not_ready: ["recovery.code.notReady", "recovery.code.notReadyHint"],
  driver_unavailable: ["recovery.code.driverUnavailable", "recovery.code.driverUnavailableHint"],
  run_ownership_unknown: ["recovery.code.runOwnership", "recovery.code.runOwnershipHint"],
  run_interrupting: ["recovery.code.interrupting", "recovery.code.interruptingHint"],
  approval_pending: ["recovery.code.approvalPending", "recovery.code.approvalPendingHint"],
  session_unavailable: ["recovery.code.sessionUnavailable", "recovery.code.sessionUnavailableHint"],
};

export function RecoveryCenter({ client, onBack, onOpenTask }: {
  client: CoreClient; onBack: () => void; onOpenTask: (taskId: TaskId) => void;
}) {
  const [snapshot, setSnapshot] = useState<RecoverySnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const refresh = useCallback(async () => {
    setBusy(true);
    try { setSnapshot(await client.recoverySnapshot()); setNotice(""); }
    catch { setNotice(t("recovery.loadFailed")); }
    finally { setBusy(false); }
  }, [client]);
  useEffect(() => { void refresh(); }, [refresh]);
  const cancel = async (item: RecoveryItem) => {
    setBusy(true);
    try {
      const current = await client.bootstrap();
      const task = current.tasks.find((task) => task.taskId === item.taskId);
      if (!task || task.activePrimaryRun?.runId !== item.runId || task.streamVersion !== item.expectedStreamVersion) {
        setNotice(t("recovery.runChanged")); return;
      }
      await client.cancelTurn(task);
      await refresh();
    } catch { setNotice(t("recovery.stopRejected")); }
    finally { setBusy(false); }
  };
  const rebuild = async (item: RecoveryItem) => {
    if (!item.taskId || item.expectedStreamVersion == null || !client.prepareSessionRebuild) return;
    setBusy(true);
    try { await client.prepareSessionRebuild(item.taskId, item.expectedStreamVersion); await refresh(); setNotice(t("recovery.rebuildReady")); }
    catch { setNotice(t("recovery.rebuildFailed")); }
    finally {setBusy(false);}
  };
  const exportSummary = async () => {
    setBusy(true);
    try { setNotice(t(await client.exportDiagnostics() === "saved" ? "recovery.exportSaved" : "recovery.exportCancelled")); }
    catch { setNotice(t("recovery.exportFailed")); }
    finally { setBusy(false); }
  };
  return <section className="workspace quota-center">
    <div className="quota-center-content">
      <button type="button" onClick={onBack}>← {t("recovery.back")}</button>
      <div className="recovery-page-heading"><h2>{t("recovery.title")}</h2><div className="task-tools">
        <button type="button" disabled={busy} onClick={() => void refresh()}>{t("recovery.refresh")}</button>
        <button type="button" disabled={busy || !snapshot} onClick={() => void exportSummary()}>{t("recovery.export")}</button>
      </div></div>
      <p>{t("recovery.exportPrivacy")}</p>
      {notice && <p role="status">{notice}</p>}
      {snapshot && <p className="quota-source">{t("recovery.checkedAt", { time: new Date(snapshot.generatedAt).toLocaleString(), count: snapshot.items.length })}
        {snapshot.truncated && ` · ${t("recovery.truncated")}`}</p>}
      <div className="quota-panel">
        {snapshot?.items.map((item, index) => <section key={index}>
          <h3>{t(recoveryMessages[item.code][0])}{item.providerId && ` · ${item.providerId}`}</h3>
          <p>{t(recoveryMessages[item.code][1])}</p>
          {item.taskId && <p className="quota-source">{t("recovery.task", { id: item.taskId.slice(-8) })}</p>}
          <div className="task-tools">
            {item.taskId && <button type="button" onClick={() => onOpenTask(item.taskId!)}>{t("recovery.openTask")}</button>}
            {item.code === "session_unavailable" && !item.runId && client.prepareSessionRebuild && <button type="button" disabled={busy} onClick={() => void rebuild(item)}>{t("recovery.rebuild")}</button>}
            {item.canCancel && <button type="button" disabled={busy} onClick={() => void cancel(item)}>{t("recovery.stopRun")}</button>}
          </div>
        </section>)}
      </div>
      {snapshot?.items.length === 0 && <p>{t("recovery.nothingFound")}</p>}
      {!snapshot && busy && <LoadingStatus layout="panel">{t("recovery.checking")}</LoadingStatus>}
    </div>
  </section>;
}
