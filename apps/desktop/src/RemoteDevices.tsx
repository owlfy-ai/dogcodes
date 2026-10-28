import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import QRCode from "qrcode";
import { LoadingStatus } from "./LoadingStatus";
import "./remoteDevices.css";

type Status = {
  service?: string;
  connected: boolean;
  configured: boolean;
  error?: string;
  invitation?: { uri: string; expiresAt: number };
  devices: { deviceId: string; publicKey: string; revoked: boolean }[];
};

export function RemoteDevices({ onClose }: { onClose: () => void }) {
  const dialogId = useRef("");
  const close = useRef(onClose);
  close.current = onClose;
  const [service, setService] = useState("http://127.0.0.1:8787");
  const [status, setStatus] = useState<Status>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [qr, setQr] = useState("");
  const [copied, setCopied] = useState(false);
  const [time, setTime] = useState(Date.now());

  async function call(request: Record<string, unknown>) {
    setBusy(true);
    setError("");
    try {
      setStatus(await invoke<Status>("core_remote_control", { call: { ...request, dialogId: dialogId.current } }));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
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
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") close.current(); };
    document.addEventListener("keydown", onKeyDown);
    return () => {
      closed = true;
      clearInterval(timer);
      document.removeEventListener("keydown", onKeyDown);
      void invoke("core_remote_control", { call: { type: "cancel_invitation", dialogId: id } }).catch(() => {});
    };
    // This effect owns a single pairing dialog; callback identity changes must not cancel it.
  }, []);

  useEffect(() => {
    let closed = false;
    setQr("");
    setCopied(false);
    if (status?.invitation) {
      void QRCode.toDataURL(status.invitation.uri, { width: 300, margin: 2, errorCorrectionLevel: "M" })
        .then(value => { if (!closed) setQr(value); })
        .catch(() => { if (!closed) setError("无法显示配对二维码，请重试。"); });
    }
    return () => { closed = true; };
  }, [status?.invitation?.uri]);

  async function copyInvitation() {
    if (!status?.invitation) return;
    try {
      await navigator.clipboard.writeText(status.invitation.uri);
      setCopied(true);
    } catch {
      setError("无法复制配对链接，请扫描二维码。");
    }
  }

  const active = status?.invitation && status.invitation.expiresAt * 1000 > time;
  const devices = status?.devices.filter(device => !device.revoked) ?? [];
  return <div className="remote-modal-backdrop" onMouseDown={event => {
    if (event.target === event.currentTarget) onClose();
  }}>
    <section className="remote-modal" role="dialog" aria-modal="true" aria-label="连接手机">
      <header>
        <h2>连接手机</h2>
        <button type="button" onClick={onClose} aria-label="关闭">×</button>
      </header>
      <p>用手机 App 扫码，绑定后可访问这台电脑的 Agent、项目与终端，无需每次确认。可随时在这里撤销。</p>
      <label>接入服务
        <input value={service} disabled={busy || status?.configured}
          onChange={event => setService(event.target.value)} placeholder="https://…" />
      </label>
      {status?.configured && <p>{status.connected ? "已连接接入服务" : (error || status.error) ? "未连接接入服务" : "正在连接接入服务…"}</p>}
      <button type="button" disabled={busy} onClick={() => void call({ type: "invite", service })}>
        {status?.invitation ? "重新生成二维码" : "显示配对二维码"}
      </button>
      {busy && <LoadingStatus>正在处理</LoadingStatus>}
      {active && qr && <div className="remote-qr">
        <img src={qr} alt="手机配对二维码" />
        <small>有效期至 {new Date(status!.invitation!.expiresAt * 1000).toLocaleTimeString()}。</small>
        <button type="button" onClick={() => void copyInvitation()}>
          {copied ? "已复制配对链接" : "复制配对链接"}
        </button>
      </div>}
      {status?.invitation && !active && <p>二维码已过期，请重新生成。</p>}
      {(error || status?.error) && <p role="alert">{error || status?.error}</p>}
      <h3>已配对设备</h3>
      {devices.length ? devices.map(device => <div className="remote-device" key={device.deviceId}>
        <span>手机 · {device.publicKey.slice(0, 12)}</span>
        <button type="button" disabled={busy}
          onClick={() => void call({ type: "revoke", deviceId: device.deviceId })}>撤销访问</button>
      </div>) : <p>尚未绑定手机。</p>}
    </section>
  </div>;
}
