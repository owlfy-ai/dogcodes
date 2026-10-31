import { useEffect, useId, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FloatingPopover } from "./FloatingPopover";
import { DropdownChevron } from "./DropdownChevron";
import { userFacingError } from "./userFacingError";
import "./openFile.css";
import { translate as t } from "./i18n";

interface Application { id: string; name: string; isDefault: boolean; icon?: string | null }
interface FileApplications { absolutePath: string; applications: Application[]; error: string | null }

async function prepareApplications(apps: Application[]): Promise<Application[]> {
  return Promise.all(apps.map(async app => {
    if (!app.icon?.startsWith("data:image/png;base64,")) return { ...app, icon: null };
    const image = new Image();
    image.src = app.icon;
    try { await image.decode?.(); }
    catch { return { ...app, icon: null }; }
    return app;
  }));
}

function ApplicationIcon({ icon }: { icon?: string | null }) {
  const [failed, setFailed] = useState(false);
  return icon && !failed
    ? <img className="open-file-app-icon" src={icon} alt="" draggable={false} onError={() => setFailed(true)} />
    : <svg className="open-file-app-icon open-file-app-placeholder" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="4" /><path d="M3 9h18M7 6.5h.01M10 6.5h.01" /></svg>;
}

export function OpenFileButton({ taskId, path, disabled = false }: { taskId: string; path: string; disabled?: boolean }) {
  const name = path.split(/[\\/]/).pop() || path;
  const [open, setOpen] = useState(false);
  const key = JSON.stringify([taskId, path]);
  const [result, setResult] = useState<{ key: string; apps: Application[]; error: string; absolutePath: string | null } | null>(null);
  const current = result?.key === key ? result : null;
  const visible = open && !disabled && current !== null;
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState(0);
  const host = useRef<HTMLSpanElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const generation = useRef(0);
  const id = useId();
  // macOS localizes Finder's name by the system language, not the app's.
  const fileManager = /Mac/.test(navigator.platform)
    ? /^zh(?:-CN|-SG|-Hans|$)/i.test(navigator.language) ? "finderLocalized" : "finder"
    : /Win/.test(navigator.platform) ? "explorer" : "fileManager";
  const close = () => { setOpen(false); trigger.current?.focus(); };
  const toggle = () => {
    if (!open && current?.error) setRetry(value => value + 1);
    setOpen(value => !value);
  };

  useEffect(() => {
    generation.current++;
    setOpen(false); setError(""); setBusy(false);
    return () => { generation.current++; };
  }, [taskId, path, disabled]);
  // Preload when the file card mounts and retain the result across openings.
  // If clicked before it is ready, show the complete menu once, with no
  // loading-to-results resize. Dismissal also cancels that pending opening.
  useEffect(() => {
    if (disabled) return;
    let valid = true;
    // Resolve the absolute path before the click so clipboard writes retain
    // their user gesture in WebKit, including when the artifact path is relative.
    void invoke<FileApplications>("core_workspace_file_applications", { taskId, path }).then(async value => {
      if (!value?.absolutePath || !Array.isArray(value.applications)) throw new Error(t("openFile.updated"));
      const apps = await prepareApplications(value.applications);
      if (valid) setResult({ key, apps, absolutePath: value.absolutePath, error: value.error ? userFacingError(value.error, t("openFile.appsFailed")) : "" });
    }).catch(cause => {
      if (valid) setResult({ key, apps: [], absolutePath: null, error: userFacingError(cause, t("openFile.pathRetry")) });
    });
    return () => { valid = false; };
  }, [disabled, taskId, path, key, retry]);
  useEffect(() => {
    if (visible) host.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
  }, [visible]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!host.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener("pointerdown", outside);
    return () => { document.removeEventListener("pointerdown", outside); };
  }, [open]);

  async function launch(applicationId?: string) {
    if (busy || disabled) return;
    const version = generation.current;
    setBusy(true); setError(""); setOpen(false);
    trigger.current?.focus();
    try { await invoke("core_open_workspace_file", { taskId, path, ...(applicationId ? { applicationId } : {}) }); }
    catch (cause) { if (generation.current === version) { setError(userFacingError(cause, t("openFile.openFailed"))); setRetry(value => value + 1); } }
    finally { if (generation.current === version) setBusy(false); }
  }

  async function fileAction(action: "reveal" | "copy") {
    if (busy || disabled) return;
    const version = generation.current;
    setBusy(true); setError(""); close();
    try {
      if (action === "copy") {
        if (!current?.absolutePath) throw new Error(t("openFile.pathFailed"));
        await navigator.clipboard.writeText(current.absolutePath);
      } else {
        await invoke("core_reveal_workspace_file", { taskId, path });
      }
    } catch (cause) {
      if (generation.current === version) setError(userFacingError(cause, t(action === "copy" ? "openFile.copyFailed" : "openFile.revealFailed")));
    } finally { if (generation.current === version) setBusy(false); }
  }

  return <span className="open-file-control" ref={host} onBlur={event => {
    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  }} onKeyDown={event => {
    if (event.key === "Escape" && open) { event.preventDefault(); event.stopPropagation(); close(); }
  }}>
    <button type="button" disabled={disabled || busy} aria-label={t("inlineEdit.openName", { name })} data-tooltip={t("openFile.defaultTooltip")} onClick={() => void launch()}>{t(busy ? "openFile.opening" : "openFile.open")}</button>
    <button type="button" ref={trigger} disabled={disabled || busy} aria-label={t("openFile.openWith", { name })} data-tooltip={t("openFile.chooseApp")}
      aria-haspopup="menu" aria-expanded={visible} aria-busy={open && !current || undefined} aria-controls={visible ? id : undefined} onClick={toggle}
      onKeyDown={event => { if (event.key === "ArrowDown") { event.preventDefault(); if (!open) toggle(); else host.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus(); } }}><DropdownChevron /></button>
    {visible && <FloatingPopover id={id} className="open-file-menu" layout="column" role="menu" aria-label={t("openFile.openWith", { name })} onKeyDown={event => {
      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
      event.preventDefault(); event.stopPropagation();
      const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
      const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
      buttons[event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : (current + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length]?.focus();
    }}>
      <div className="open-file-apps" data-popover-scroll="">
      <button type="button" role="menuitem" onClick={() => void launch()}>{t("openFile.defaultApp")}</button>
      <hr />
      {current.apps.map(app => <button type="button" role="menuitem" key={app.id} onClick={() => void launch(app.id)}><span className="open-file-app-label"><ApplicationIcon key={app.icon} icon={app.icon} /><span>{app.name}</span></span>{app.isDefault && <small>{t("modelPicker.default")}</small>}</button>)}
      {current.apps.length === 0 && !current.error && <p>{t("openFile.noApps")}</p>}
      {current.error && <><p role="alert">{current.error}</p><button type="button" role="menuitem" onClick={() => setRetry(value => value + 1)}>{t("openFile.requery")}</button></>}
      </div>
      <div className="open-file-footer">
        <button type="button" role="menuitem" onClick={() => void fileAction("reveal")}><span className="open-file-app-label"><svg className="open-file-app-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" /></svg><span>{t(`openFile.reveal.${fileManager}`)}</span></span></button>
        <button type="button" role="menuitem" disabled={!current.absolutePath} data-tooltip={!current.absolutePath ? current.error : undefined} onClick={() => void fileAction("copy")}><span className="open-file-app-label"><svg className="open-file-app-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><rect x="8" y="8" width="12" height="13" rx="2" /><path d="M15 5V4a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h1" /></svg><span>{t("openFile.copyPath")}</span></span></button>
      </div>
    </FloatingPopover>}
    {!open && error && <span className="open-file-error" role="alert">{error}</span>}
  </span>;
}
