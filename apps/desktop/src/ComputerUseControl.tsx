import { useEffect, useState } from "react";
import type { ComputerUseStatus, CoreClient, TaskId } from "@dogcodes/protocol";
import { useI18n } from "./i18n";
import { LoadingStatus } from "./LoadingStatus";
import { invoke, isTauri } from "@tauri-apps/api/core";

export function ComputerUseControl({ client, taskId, running }: { client: CoreClient; taskId: TaskId; running: boolean }) {
  const { t } = useI18n();
  const [status, setStatus] = useState<ComputerUseStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let disposed = false;
    setStatus(null);
    setError("");
    if (client.computerUse) void client.computerUse(taskId, { type: "status" }).then(value => { if (!disposed) setStatus(value); }).catch(reason => { if (!disposed) setError(String(reason)); });
    return () => { disposed = true; };
  }, [client, taskId, running]);
  if (!client.computerUse) return null;
  const label = status?.enabled ? t("computerUse.disable") : t("computerUse.enable");
  return <span className="computer-use-control">
    <button type="button" className={`composer-icon${status?.enabled ? " computer-use-enabled" : ""}`}
      aria-label={label} aria-pressed={status?.enabled ?? false}
      data-tooltip={error || (!status?.available ? t(status?.unavailableReason === "macos_version" ? "computerUse.macosVersion" : "computerUse.unavailable") : running && !status.enabled ? t("computerUse.nextTurn") : status.enabled ? t("computerUse.enabledHint") : t("computerUse.enableHint"))}
      disabled={pending || !status?.available || (running && !status.enabled)}
      onClick={() => {
        if (!status || !client.computerUse) return;
        setPending(true); setError("");
        void (async () => {
          if (!status.enabled && isTauri()) {
            const permissions = await invoke<{required:boolean;accessibility:boolean;screenRecording:boolean}>("computer_use_permissions", {prompt:true});
            if (permissions.required && (!permissions.accessibility || !permissions.screenRecording)) throw new Error(t("computerUse.permissions"));
          }
          return client.computerUse!(taskId, { type: "set", enabled: !status.enabled });
        })().then(setStatus).catch(reason => setError(String(reason))).finally(() => setPending(false));
      }}>
      {pending ? <LoadingStatus><span className="sr-only">{label}</span></LoadingStatus> : <svg viewBox="0 0 20 20" aria-hidden="true"><rect x="2.5" y="3" width="15" height="10" rx="1.5"/><path d="M10 13v4M6.5 17h7"/></svg>}
    </button>
    {error && <span className="sr-only" role="alert">{error}</span>}
  </span>;
}
