import { SharedHistoryPreferencesGate } from "./SharedHistoryPreferencesGate";
import { lazy, Suspense, useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { StartupShell } from "./StartupShell";
import { SharedAgentPreferencesGate } from "./SharedAgentPreferencesGate";
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
      const messages: Record<string, string> = {storage_directory_unavailable:"无法访问应用数据目录。",host_lease_unavailable:"应用可能已在其他窗口运行，请关闭重复打开的应用后重试。",database_check_failed:"本地数据暂时无法读取，请先备份再检查磁盘空间和访问权限。",database_from_newer_version:"本地数据由更新版本的 dog.codes 写入，当前版本不会读取或修改它。请安装该版本或更新的版本后再打开。",scratch_directory_unavailable:"无法创建临时文件，请检查磁盘空间和访问权限。",history_recovery_failed:"上次对话的恢复未完成，请保留数据并导出问题报告。"};
      if (active) setReason(messages[value.code] ?? "本地数据暂时无法读取。");
    }).catch(() => {});
    return () => {active = false;};
  }, [status]);
  const exportSummary = async () => {
    setBusy("export"); setNotice("");
    try { const result = await invoke<string>("core_export_startup_diagnostics"); if (result === "saved") setNotice("问题报告已保存。"); }
    catch {setNotice("报告未能保存，请换一个文件夹重试。");}
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
    catch { setNotice("请退出应用后重新打开。"); setBusy(null); }
  };
  return <StartupShell><div className="startup-recovery">
    <div className="startup-symbol" aria-hidden="true">C</div>
    <h1>{storage ? "暂时无法打开本地数据" : "暂时无法打开对话"}</h1>
    <p>{storage ? "为保护已有记录，已暂停打开对话。请先保留本地数据，再按下方说明处理。" : "连接暂时不可用。请重新尝试；如果仍无法打开，请退出后重新打开应用。"}</p>
    {status === "service_unavailable" && <p>正在进行的任务可能仍在继续，重新打开不会重复发送消息。</p>}
    {reason && <p>{reason}</p>}
    <div className="startup-actions">
      {!storage && <button type="button" disabled={Boolean(busy)} onClick={() => void retry()}>{restart ? "重新打开应用" : "重新尝试"}</button>}
      {isTauri() && <button type="button" disabled={Boolean(busy)} onClick={() => void exportSummary()}>{busy === "export" ? "正在保存…" : "保存问题报告"}</button>}
    </div>
    <p className="startup-notice" role="status">{notice}</p>
    <details><summary>{storage ? "查看处理步骤" : "查看问题报告说明"}</summary>
      {storage && <><p>关闭应用后，完整备份应用数据目录（包括 app-v5.db、存在的 app-v5.db-wal 和 app-v5.db-shm）。</p><p>检查磁盘空间、访问权限及应用版本，再重新打开应用。请保留原文件，不要直接删除数据库。应用不会自动修复数据或重发指令。</p></>}
      <p>问题报告包含启动状态、版本和后台服务诊断，已按规则脱敏；分享前请检查报告内容。</p>
    </details>
  </div></StartupShell>;
}
