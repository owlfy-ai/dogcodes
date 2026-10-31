import { SharedHistoryPreferencesGate } from "./SharedHistoryPreferencesGate";
import { lazy, Suspense, useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { StartupShell } from "./StartupShell";
import { SharedAgentPreferencesGate } from "./SharedAgentPreferencesGate";
import { translate as t } from "./i18n";
const loadApp = () => import("./App").then(({ App }) => ({ default: App }));
const ConversationApp = lazy(loadApp);

type StartupStatus = "ready" | "storage_recovery_required" | "service_unavailable";

export function StartupGate() {
  const [status, setStatus] = useState<StartupStatus | "loading" | "unavailable">("loading");
  useEffect(() => {
    let active = true;
    // Load code while the service check runs, rather than serializing both waits.
    void loadApp().catch(() => { if (active) setStatus("unavailable"); });
    if (!isTauri()) { setStatus("ready"); return () => { active = false; }; }
    void invoke<StartupStatus>("core_startup_status")
      .then((value) => { if (active) setStatus(value === "ready" || value === "service_unavailable" || value === "storage_recovery_required" ? value : "unavailable"); })
      .catch(() => { if (active) setStatus("unavailable"); });
    return () => { active = false; };
  }, []);
  if (status === "ready") return <SharedAgentPreferencesGate><SharedHistoryPreferencesGate><Suspense fallback={<StartupShell />}><ConversationApp /></Suspense></SharedHistoryPreferencesGate></SharedAgentPreferencesGate>;
  return <StartupRecovery status={status} />;
}

export function StartupRecovery({ status, onRetry }: { status: "loading" | "unavailable" | "storage_recovery_required" | "service_unavailable"; onRetry?: () => void }) {
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState<"export" | "restart" | null>(null);
  const [reason, setReason] = useState("");
  useEffect(() => {
    let active = true;
    if (status === "storage_recovery_required" && isTauri()) void invoke<{code: string}>("core_startup_diagnostics").then(value => {
      const messages: Record<string, string> = {storage_directory_unavailable: "storageDirectory", host_lease_unavailable: "hostLease", database_check_failed: "databaseCheck", database_from_newer_version: "newerVersion", scratch_directory_unavailable: "scratchDirectory", history_recovery_failed: "historyRecovery"};
      if (active) setReason(t(`startup.reason.${messages[value.code] ?? "unreadable"}`));
    }).catch(() => {});
    return () => {active = false;};
  }, [status]);
  const exportSummary = async () => {
    setBusy("export"); setNotice("");
    try { const result = await invoke<string>("core_export_startup_diagnostics"); if (result === "saved") setNotice(t("startup.reportSaved")); }
    catch {setNotice(t("startup.reportFailed"));}
    finally {setBusy(null);}
  };
  if (status === "loading") return <StartupShell />;
  const storage = status === "storage_recovery_required";
  const restart = status === "service_unavailable" && isTauri();
  const retry = async () => {
    if (onRetry) { onRetry(); return; }
    if (!restart) { window.location.reload(); return; }
    setBusy("restart"); setNotice("");
    try { await invoke("restart_application"); }
    catch { setNotice(t("startup.reopen")); setBusy(null); }
  };
  return <StartupShell><div className="startup-recovery">
    <div className="startup-symbol" aria-hidden="true">C</div>
    <h1>{t(storage ? "startup.storageTitle" : "startup.title")}</h1>
    <p>{t(storage ? "startup.storageDescription" : "startup.description")}</p>
    {status === "service_unavailable" && <p>{t("startup.tasksContinue")}</p>}
    {reason && <p>{reason}</p>}
    <div className="startup-actions">
      {!storage && <button type="button" disabled={Boolean(busy)} onClick={() => void retry()}>{t(restart ? "startup.restart" : "startup.retry")}</button>}
      {isTauri() && <button type="button" disabled={Boolean(busy)} onClick={() => void exportSummary()}>{t(busy === "export" ? "common.saving" : "startup.saveReport")}</button>}
    </div>
    <p className="startup-notice" role="status">{notice}</p>
    <details><summary>{t(storage ? "startup.stepsSummary" : "startup.reportSummary")}</summary>
      {storage && <><p>{t("startup.backupStep")}</p><p>{t("startup.checkStep")}</p></>}
      <p>{t("startup.reportContents")}</p>
    </details>
  </div></StartupShell>;
}
