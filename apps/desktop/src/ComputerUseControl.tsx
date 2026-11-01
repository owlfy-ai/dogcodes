import { useEffect, useState } from "react";
import type { ComputerUseStatus, CoreClient } from "@dogcodes/protocol";
import { useI18n } from "./i18n";
import { LoadingStatus } from "./LoadingStatus";
import { invoke, isTauri } from "@tauri-apps/api/core";

export function ComputerUseControl({ client }: { client: CoreClient }) {
  const { t } = useI18n();
  const [status, setStatus] = useState<ComputerUseStatus | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let disposed = false;
    setStatus(null);
    setError("");
    if (client.computerUse) void client.computerUse({ type: "status" }).then(value => { if (!disposed) setStatus(value); }).catch(reason => { if (!disposed) setError(String(reason)); });
    return () => { disposed = true; };
  }, [client]);
  if (!client.computerUse) return null;
  const label = t("computerUse.menuLabel");
  return <>
    <button type="button" role="switch" className="computer-use-settings-toggle" aria-label={label} aria-checked={status?.enabled ?? false}
      data-tooltip={error || (!status?.available ? t(status?.unavailableReason === "macos_version" ? "computerUse.macosVersion" : "computerUse.unavailable") : status.enabled ? t("computerUse.enabledHint") : t("computerUse.enableHint"))}
      disabled={pending || !status || (!status.available && !status.enabled)}
      onClick={() => {
        if (!status || !client.computerUse) return;
        setPending(true); setError("");
        void (async () => {
          if (!status.enabled && isTauri()) {
            const permissions = await invoke<{required:boolean;accessibility:boolean;screenRecording:boolean}>("computer_use_permissions", {prompt:true});
            if (permissions.required && (!permissions.accessibility || !permissions.screenRecording)) throw new Error(t("computerUse.permissions"));
          }
          return client.computerUse!({ type: "set", enabled: !status.enabled });
        })().then(setStatus).catch(reason => setError(String(reason))).finally(() => setPending(false));
      }}>
      <svg className="preferences-icon" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><rect x="2.5" y="3" width="15" height="10" rx="1.5"/><path d="M10 13v4M6.5 17h7"/></svg>
      <span>{label}</span>
      {pending || !status && !error ? <LoadingStatus><span className="sr-only">{label}</span></LoadingStatus> : <span className="computer-use-switch" aria-hidden="true"><span /></span>}
    </button>
    {error && <p className="computer-use-settings-error" role="alert">{error}</p>}
  </>;
}
