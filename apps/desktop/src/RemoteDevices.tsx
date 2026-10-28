import { useEffect, useId, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import QRCode from "qrcode";
import { LoadingStatus } from "./LoadingStatus";
import { SidebarNavIcon } from "./SidebarNavIcon";
import { t, useI18n, type ResolvedLanguage } from "./i18n";
import "./remoteDevices.css";

type Status = {
  service?: string;
  connected: boolean;
  configured: boolean;
  error?: string;
  invitation?: { uri: string; expiresAt: number };
  devices: { deviceId: string; publicKey: string; revoked: boolean }[];
};
type LocalError = "qrFailed" | "copyFailed";

function PairingIcon({ kind }: { kind: "qr" | "shield" | "copy" }) {
  return <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {kind === "qr" ? <><rect x="3" y="3" width="6" height="6" rx="1" /><rect x="15" y="3" width="6" height="6" rx="1" /><rect x="3" y="15" width="6" height="6" rx="1" /><path d="M15 15h3v3h3m-6 3h3m3-9v3M3 12h3m6-9v3m0 6v6" /></>
      : kind === "shield" ? <><path d="m12 3 8 3v6c0 4-4 7-8 9-4-2-8-5-8-9V6z" /><path d="m8.5 12 2.5 2.5 4.5-5" /></>
      : <><rect x="8" y="8" width="12" height="13" rx="2" /><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3" /></>}
  </svg>;
}

export function RemoteDevices({ lang }: { lang?: ResolvedLanguage }) {
  const { resolvedLanguage } = useI18n();
  const language = lang ?? resolvedLanguage;
  const tr = (key: string, params?: Record<string, string | number>) => t(`remoteDevices.${key}`, params, language);
  const descriptionId = useId();
  const serviceId = useId();
  const dialogId = useRef("");
  const mounted = useRef(false);
  const [service, setService] = useState("http://127.0.0.1:8787");
  const [status, setStatus] = useState<Status>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [localError, setLocalError] = useState<LocalError>();
  const [qr, setQr] = useState("");
  const [copied, setCopied] = useState(false);
  const [time, setTime] = useState(Date.now());

  async function call(request: Record<string, unknown>) {
    setBusy(true);
    setError("");
    setLocalError(undefined);
    try {
      const result = await invoke<Status>("core_remote_control", { call: { ...request, dialogId: dialogId.current } });
      if (mounted.current) setStatus(result);
    } catch (cause) {
      if (mounted.current) setError(String(cause));
    } finally {
      if (mounted.current) setBusy(false);
    }
  }

  useEffect(() => {
    mounted.current = true;
    let closed = false;
    const id = crypto.randomUUID();
    dialogId.current = id;
    const refresh = () => {
      void invoke<Status>("core_remote_control", { call: { type: "status" } }).then(value => {
        if (closed) return;
        setStatus(value);
        if (value.service) setService(value.service);
      }).catch(cause => { if (!closed) setError(String(cause)); });
    };
    refresh();
    const timer = setInterval(() => { refresh(); setTime(Date.now()); }, 1500);
    return () => {
      mounted.current = false;
      closed = true;
      clearInterval(timer);
      void invoke("core_remote_control", { call: { type: "cancel_invitation", dialogId: id } }).catch(() => {});
    };
    // Keep the invitation while on this page; leaving cancels only this page's invitation.
  }, []);

  useEffect(() => {
    let closed = false;
    setQr("");
    setCopied(false);
    setLocalError(undefined);
    if (status?.invitation) {
      void QRCode.toDataURL(status.invitation.uri, { width: 300, margin: 2, errorCorrectionLevel: "M" })
        .then(value => { if (!closed) setQr(value); })
        .catch(() => { if (!closed) setLocalError("qrFailed"); });
    }
    return () => { closed = true; };
  }, [status?.invitation?.uri]);

  async function copyInvitation() {
    if (!status?.invitation || status.invitation.expiresAt * 1000 <= Date.now()) return;
    try {
      await navigator.clipboard.writeText(status.invitation.uri);
      if (mounted.current) { setCopied(true); setLocalError(undefined); }
    } catch {
      if (mounted.current) setLocalError("copyFailed");
    }
  }

  const active = status?.invitation && status.invitation.expiresAt * 1000 > time;
  const devices = status?.devices.filter(device => !device.revoked) ?? [];
  const failure = error || status?.error;
  const connection = status?.connected ? "connected" : failure ? "disconnected" : "connecting";
  const expiry = status?.invitation ? new Intl.DateTimeFormat(language, { hour: "numeric", minute: "2-digit" }).format(status.invitation.expiresAt * 1000) : "";

  return <section className="workspace remote-page" aria-label={tr("title")} aria-describedby={descriptionId}>
    <div className="remote-page-content">
      <header className="remote-header">
        <span className="remote-heading-icon"><SidebarNavIcon kind="phone" /></span>
        <p id={descriptionId}>{tr("subtitle")}</p>
      </header>
      <div className="remote-pairing-layout">
        <div className="remote-setup">
          <h2>{tr("setupTitle")}</h2>
          <p className="remote-description">{tr("description")}</p>
          <ol className="remote-steps"><li>{tr("stepOpen")}</li><li>{tr("stepScan")}</li></ol>
          <label className="remote-service-label" htmlFor={serviceId}>{tr("service")}</label>
          <input id={serviceId} className="remote-service-input" value={service} disabled={busy || status?.configured || !status}
            onChange={event => setService(event.target.value)} placeholder="https://…" spellCheck={false} autoCapitalize="none" autoComplete="off" />
          {status?.configured && <p className={`remote-connection ${connection}`} role="status"><span aria-hidden="true" />{tr(connection)}</p>}
          <button className="remote-primary" type="button" disabled={busy || !status || !service.trim()} onClick={() => void call({ type: "invite", service })}>
            <PairingIcon kind="qr" />{busy ? <LoadingStatus>{tr("processing")}</LoadingStatus> : tr(status?.invitation ? "regenerate" : "generate")}
          </button>
          {!status && !failure && <LoadingStatus>{tr("loading")}</LoadingStatus>}
          <p className="remote-trust-note"><PairingIcon kind="shield" />{tr("trustNote")}</p>
        </div>
        <div className={`remote-qr-panel${active && qr ? " ready" : ""}`}>
          {active && qr ? <>
            <div className="remote-qr-image"><img src={qr} alt={tr("qrAlt")} /></div>
            <strong>{tr("scanTitle")}</strong>
            <small>{tr("expires", { time: expiry })}</small>
          </> : <>
            <div className="remote-qr-placeholder"><PairingIcon kind="qr" /></div>
            <strong>{tr(status?.invitation && !active ? "expiredTitle" : "qrPlaceholder")}</strong>
            <p>{tr(status?.invitation && !active ? "expired" : "qrHint")}</p>
          </>}
          {active && <button className="remote-copy" type="button" onClick={() => void copyInvitation()}><PairingIcon kind="copy" /><span role="status">{tr(copied ? "copied" : "copy")}</span></button>}
        </div>
      </div>
      {(failure || localError) && <div className="remote-error" role="alert"><strong>{tr(localError ?? "requestFailed")}</strong>{failure && <p>{failure}</p>}</div>}
      <section className="remote-devices" aria-label={tr("pairedDevices")}>
        <header><h2>{tr("pairedDevices")}</h2>{status && <span className="remote-device-count">{devices.length}</span>}</header>
        {devices.length ? <ul>{devices.map(device => <li className="remote-device" key={device.deviceId}>
          <span className="remote-device-icon"><SidebarNavIcon kind="phone" /></span>
          <div className="remote-device-identity"><strong>{tr("phone")}</strong><span>{device.publicKey.slice(0, 12)}</span></div>
          <button className="remote-revoke" type="button" disabled={busy} aria-label={tr("revokeDevice", { id: device.publicKey.slice(0, 12) })}
            onClick={() => void call({ type: "revoke", deviceId: device.deviceId })}>{tr("revoke")}</button>
        </li>)}</ul> : <p className="remote-empty">{tr(status ? "emptyDevices" : "loading")}</p>}
      </section>
    </div>
  </section>;
}
