import { type FormEvent, type ReactNode, useEffect, useId, useRef, useState } from "react";
import { ConfirmDialog } from "./AppDialog";
import { LoadingStatus } from "./LoadingStatus";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { SidebarNavIcon } from "./SidebarNavIcon";
import { focusDialogTitle } from "./dialogFocus";
import { useI18n } from "./i18n";
import { userFacingError } from "./userFacingError";
import {
  type SshAuth, type TargetApi, type TargetEntry, type TargetErrorKind, type TargetStatus,
  connectionFromForm, joinDirectory, parentDirectory, stepStates, tauriTargetApi, targetRoute,
} from "./executionTargetClient";
import { needsPerson, useTargetConnection } from "./targetConnection";
import "./executionTargets.css";

const POLL_MILLISECONDS = 400;
const ERROR_KEYS: Record<TargetErrorKind, string> = {
  ssh_missing: "errorSshMissing", ssh_too_old: "errorSshTooOld", auth_failed: "errorAuthFailed",
  host_key_changed: "errorHostKeyChanged", host_key_rejected: "errorHostKeyRejected",
  host_unreachable: "errorHostUnreachable", connection_closed: "errorConnectionClosed", timeout: "errorTimeout", cancelled: "errorCancelled",
  account_restricted: "errorAccountRestricted", unsupported_platform: "errorUnsupportedPlatform",
  shim_unavailable: "errorShimUnavailable", home_missing: "errorHomeMissing",
  directory_unwritable: "errorDirectoryUnwritable", unsafe_directory: "errorUnsafeDirectory",
  upload_failed: "errorUploadFailed", not_executable: "errorNotExecutable", bad_binary: "errorBadBinary",
  protocol_mismatch: "errorProtocolMismatch", failed: "errorFailed",
};

function useTargetText() {
  const { t } = useI18n();
  return (key: string, params?: Record<string, string | number>) => t(`executionTargets.${key}`, params);
}

const AGENT_STATUS_KEYS = { ready: "agentReady", installed: "agentNotReady", not_configured: "agentNeedsLogin", unsupported_version: "agentUnsupported", broken: "agentBroken" } as const;

function RemoteProjectDialog({ api, targetId, onClose, onReady }: {
  api: TargetApi; targetId: string; onClose: () => void; onReady: (projectId: string) => void | Promise<void>;
}) {
  const { t } = useI18n();
  const text = useTargetText();
  const [path, setPath] = useState("");
  const [typed, setTyped] = useState("");
  const [directories, setDirectories] = useState<string[] | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const open = async (next: string | null) => {
    setMessage("");
    try {
      const listing = await api.browse(targetId, next);
      setPath(listing.path);
      setTyped(listing.path);
      setDirectories(listing.directories);
    } catch (cause) {
      setMessage(userFacingError(cause, text("browseFailed")));
    }
  };
  useEffect(() => { void open(null); }, [api, targetId]);
  const create = async () => {
    if (busy || !path) return;
    setBusy(true);
    setMessage("");
    try {
      await onReady(await api.createProject(targetId, path, name));
    } catch (cause) {
      setMessage(userFacingError(cause, text("createProjectFailed")));
      setBusy(false);
    }
  };
  return <div className="target-browser">
    <form className="target-browser-path" onSubmit={(event: FormEvent) => { event.preventDefault(); void open(typed.trim()); }}>
      <input aria-label={text("browsePath")} value={typed} disabled={busy} onChange={event => setTyped(event.target.value)} />
      <button className="secondary" disabled={busy || !typed.trim()}>{text("browseGo")}</button>
    </form>
    <ul className="target-browser-list" aria-label={text("browseTitle")}>
      {path !== "/" && path && <li><button type="button" disabled={busy} onClick={() => void open(parentDirectory(path))}>..</button></li>}
      {directories?.map(directory => <li key={directory}><button type="button" disabled={busy} onClick={() => void open(joinDirectory(path, directory))}>{directory}</button></li>)}
      {directories?.length === 0 && <li className="target-browser-empty">{text("browseEmpty")}</li>}
      {!directories && !message && <li className="target-browser-empty"><LoadingStatus>{text("loading")}</LoadingStatus></li>}
    </ul>
    <label>{text("name")}<input maxLength={120} disabled={busy} placeholder={path.split("/").filter(Boolean).pop() ?? ""} value={name} onChange={event => setName(event.target.value)} /></label>
    {message && <div className="error" role="alert">{message}</div>}
    <footer>
      <button className="secondary" type="button" disabled={busy} onClick={onClose}>{t("common.cancel")}</button>
      <button className="send" type="button" disabled={busy || !path} onClick={() => void create()}>{text("openHere")}</button>
    </footer>
  </div>;
}

/** Reconnecting from where the person is. The dialog appears only when they are needed,
 * for a password, a host key or a failure, and only in the view they clicked. */
function useReconnect(api: TargetApi, targetId: string) {
  const text = useTargetText();
  const owner = useId();
  const { state, reconnect, dismiss } = useTargetConnection(api, targetId);
  const connecting = state.status?.phase === "connecting";
  return {
    offline: state.offline,
    failure: state.failure ? userFacingError(state.failure, text("loadFailed")) : "",
    button: { disabled: connecting, label: text(connecting ? "reconnecting" : "reconnect"), onClick: () => void reconnect(owner) },
    dialog: needsPerson(state) && state.entry && state.asker === owner
      ? <TargetConnectionDialog api={api} entry={state.entry} initialProgress onClose={dismiss} onChanged={() => {}} />
      : null,
  };
}

/** Sending would fail anyway: say why and reconnect from here. */
export function TargetOfflineBanner({ api = tauriTargetApi, targetId }: {
  api?: TargetApi; targetId: string;
}) {
  const text = useTargetText();
  const { offline, failure, button, dialog } = useReconnect(api, targetId);
  if (!offline) return null;
  return <>
    <div className="target-offline" role="status">
      <span>{failure || text("offlineBanner")}</span>
      <button type="button" disabled={button.disabled} onClick={button.onClick}>{button.label}</button>
    </div>
    {dialog}
  </>;
}

/** A side panel of a project on a server: without the server there is nothing it can
 * show, so it starts afresh once connected instead of carrying errors from before. */
export function TargetOfflinePanel({ api = tauriTargetApi, targetId, visible, standalone = false, onClose, children }: {
  api?: TargetApi; targetId: string; visible: boolean; standalone?: boolean; onClose: () => void; children: ReactNode;
}) {
  const text = useTargetText();
  const { t } = useI18n();
  const { offline, failure, button, dialog } = useReconnect(api, targetId);
  if (offline === null) return null;
  if (!offline) return <>{children}</>;
  if (!visible) return null;
  return <>
    <aside className={`review-panel${standalone ? " review-standalone" : ""}`} aria-label={t("reviewPanel.title")}>
      <header className="review-titlebar" data-tauri-drag-region="deep">
        <span />
        <div className="review-window-controls"><button type="button" onClick={onClose} aria-label={t("reviewPanel.close")} data-tooltip={t(standalone ? "reviewPanel.closeWindow" : "reviewPanel.closeSidebar")}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><path d="m7 7 10 10M17 7 7 17" /></svg></button></div>
      </header>
      <ReviewEmptyState kind="folder" title={failure || text("offlineBanner")}
        action={<button type="button" className="review-empty-action" disabled={button.disabled} onClick={button.onClick}>{button.label}</button>} />
    </aside>
    {dialog}
  </>;
}

function ConnectionProgress({ api, targetId, onEdit, onClose, onBrowse }: {
  api: TargetApi; targetId: string; onEdit: () => void; onClose: () => void; onBrowse?: () => void;
}) {
  const text = useTargetText();
  const [status, setStatus] = useState<TargetStatus | null>(null);
  const [failure, setFailure] = useState("");
  const [secret, setSecret] = useState("");
  const [remember, setRemember] = useState(true);
  useEffect(() => {
    let closed = false;
    const read = () => void api.status(targetId).then(value => {
      if (!closed) { setStatus(value); setFailure(""); }
    }).catch(cause => { if (!closed) setFailure(userFacingError(cause, text("loadFailed"))); });
    read();
    const timer = setInterval(read, POLL_MILLISECONDS);
    return () => { closed = true; clearInterval(timer); };
    // The status of one target for as long as this view is open.
  }, [api, targetId]);
  const prompt = status?.prompt ?? null;
  const respond = (answer: string | null) => {
    if (!prompt) return;
    setSecret("");
    void api.answer(targetId, prompt.id, answer, remember && answer !== null && prompt.kind !== "host_key" && prompt.kind !== "other")
      .catch(cause => setFailure(userFacingError(cause, text("loadFailed"))));
  };
  if (!status) return failure ? <div className="error" role="alert">{failure}</div> : <LoadingStatus>{text("loading")}</LoadingStatus>;
  const connected = status.phase === "connected";
  const deploying = status.step?.step === "deploying" ? status.step : null;
  return <div className="target-progress">
    <ol className="target-steps">
      {stepStates(status).map(({ name, state }) => <li key={name} className={state} aria-current={state === "active" ? "step" : undefined}>
        <span aria-hidden="true">{state === "done" ? "✓" : state === "failed" ? "×" : ""}</span>
        {text(`step_${name}`)}
        {name === "deploying" && state === "active" && deploying && deploying.total > 0 && <small>{Math.round(deploying.sent / deploying.total * 100)}%</small>}
      </li>)}
    </ol>
    {prompt && (prompt.kind === "host_key"
      ? <section className="target-prompt" role="alertdialog" aria-label={text("hostKeyTitle")}>
        <strong>{text("hostKeyTitle")}</strong>
        <pre>{prompt.text}</pre>
        <p>{text("hostKeyHint")}</p>
        <div className="target-actions">
          <button className="secondary" type="button" onClick={() => respond("no")}>{text("reject")}</button>
          <button className="send" type="button" onClick={() => respond("yes")}>{text("trust")}</button>
        </div>
      </section>
      : <form className="target-prompt" onSubmit={(event: FormEvent) => { event.preventDefault(); respond(secret); }}>
        <label>{prompt.text}<input autoFocus type="password" autoComplete="off" value={secret} onChange={event => setSecret(event.target.value)} /></label>
        {(prompt.kind === "password" || prompt.kind === "passphrase") && <label className="target-check"><input type="checkbox" checked={remember} onChange={event => setRemember(event.target.checked)} />{text("remember")}</label>}
        <div className="target-actions">
          <button className="secondary" type="button" onClick={() => respond(null)}>{text("reject")}</button>
          <button className="send">{text("submit")}</button>
        </div>
      </form>)}
    {connected && status.info && <p className="target-connected" role="status">
      <strong>{text("connectedTitle")}</strong>
      {text("connectedSummary", { os: status.info.os, arch: status.info.arch, version: status.info.version })}
    </p>}
    {connected && status.identityChanged && <div className="error" role="alert">{text("identityChanged")}</div>}
    {connected && status.scanning && <LoadingStatus>{text("scanningAgents")}</LoadingStatus>}
    {connected && !status.scanning && (status.agents.length
      ? <ul className="target-agents" aria-label={text("agentsTitle")}>
        {status.agents.map(agent => <li key={agent.providerId}>
          <strong>{agent.displayName}</strong>
          {agent.version && <small>{agent.version}</small>}
          <span className={`target-agent-status ${agent.status}`}>{text(AGENT_STATUS_KEYS[agent.status])}</span>
        </li>)}
      </ul>
      : <p className="target-hint">{text("noAgents")}</p>)}
    {status.error && <div className="error" role="alert">{text(ERROR_KEYS[status.error.kind])}</div>}
    {status.phase === "disconnected" && <div className="error" role="alert">{text("connectionLost")}{status.closedReason ? ` ${status.closedReason}` : ""}</div>}
    {failure && <div className="error" role="alert">{failure}</div>}
    {status.log.length + (status.error?.detail ? 1 : 0) > 0 && <details className="target-log">
      <summary>{text("log")}</summary>
      <pre>{[...status.log, ...(status.error?.detail && !status.log.join("\n").includes(status.error.detail) ? [status.error.detail] : [])].join("\n")}</pre>
    </details>}
    <footer>
      {(status.phase === "failed" || status.phase === "disconnected") && <>
        <button className="secondary" type="button" onClick={onEdit}>{text("backToEdit")}</button>
        <button className="send" type="button" onClick={() => void api.connect(targetId, null).catch(cause => setFailure(userFacingError(cause, text("loadFailed"))))}>{text("retry")}</button>
      </>}
      {status.phase === "connecting" && !prompt && <button className="secondary" type="button" onClick={() => { void api.disconnect(targetId).catch(() => {}); onEdit(); }}>{text("cancelConnecting")}</button>}
      {connected && <button className={onBrowse ? "secondary" : "send"} type="button" onClick={onClose}>{text("done")}</button>}
      {connected && onBrowse && <button className="send" type="button" disabled={status.scanning} onClick={onBrowse}>{text("openProject")}</button>}
    </footer>
  </div>;
}

export function TargetConnectionDialog({ api, entry, initialProgress = false, initialBrowsing = false, onClose, onChanged, onProjectReady }: {
  api: TargetApi; entry: TargetEntry | null; initialProgress?: boolean; initialBrowsing?: boolean; onClose: () => void; onChanged: () => void;
  onProjectReady?: (projectId: string) => void | Promise<void>;
}) {
  const { t } = useI18n();
  const text = useTargetText();
  const saved = entry?.target.connection;
  const [targetId, setTargetId] = useState<string | null>(entry?.target.targetId ?? null);
  const [progress, setProgress] = useState(initialProgress);
  const [browsing, setBrowsing] = useState(initialBrowsing);
  const [name, setName] = useState(entry?.target.displayName ?? "");
  const [destination, setDestination] = useState(saved?.destination ?? "");
  const [port, setPort] = useState(saved?.port ? String(saved.port) : "");
  const [user, setUser] = useState(saved?.user ?? "");
  const [auth, setAuth] = useState<SshAuth>(saved?.auth ?? "system");
  const [identityFile, setIdentityFile] = useState(saved?.identityFile ?? "");
  const [secret, setSecret] = useState("");
  const [showSecret, setShowSecret] = useState(false);
  const [installDirectory, setInstallDirectory] = useState(saved?.installDirectory ?? "");
  const [hosts, setHosts] = useState<string[]>([]);
  const [resolved, setResolved] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    focusDialogTitle(dialog.current);
    void api.configHosts().then(setHosts).catch(() => {});
    return () => previous?.focus();
  }, [api]);
  const importHost = (alias: string) => {
    if (!alias) return;
    setDestination(alias);
    setName(current => current || alias);
    setPort("");
    setUser("");
    setAuth("system");
    setResolved("");
    void api.resolveHost(alias).then(host => {
      if (host) setResolved(`${host.user ? `${host.user}@` : ""}${host.host}${host.port ? `:${host.port}` : ""}`);
    }).catch(() => {});
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy || !destination.trim()) return;
    setBusy(true);
    setMessage("");
    try {
      const connection = connectionFromForm({ destination, port, user, auth, identityFile, installDirectory });
      const id = await api.save(targetId, name.trim() || destination.trim(), connection);
      setTargetId(id);
      onChanged();
      await api.disconnect(id);
      await api.connect(id, auth === "system" ? null : secret || null);
      setSecret("");
      setProgress(true);
    } catch (cause) {
      setMessage(userFacingError(cause, text("saveFailed")));
    } finally {
      setBusy(false);
    }
  };
  const close = () => { if (!busy) { onChanged(); onClose(); } };
  const title = browsing ? text("browseTitle") : progress ? name.trim() || destination.trim() : entry ? text("editTitle") : text("newTitle");
  const preview = destination.trim() ? targetRoute(connectionFromForm({ destination, port, user, auth, identityFile, installDirectory })) : "user@host:22";
  return <div className="modal-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) close(); }}>
    <div ref={dialog} className="project-dialog target-dialog" role="dialog" aria-modal="true" aria-labelledby="target-dialog-title"
      onKeyDown={event => { if (event.key === "Escape") { event.preventDefault(); close(); } }}>
      <header>
        <div><h2 id="target-dialog-title" tabIndex={-1} data-dialog-title>{title}</h2><small className="target-route">{resolved || preview}</small></div>
        <button type="button" aria-label={t("common.close")} disabled={busy} onClick={close}>×</button>
      </header>
      {browsing && targetId && onProjectReady
        ? <RemoteProjectDialog api={api} targetId={targetId} onClose={() => setBrowsing(false)} onReady={async projectId => { await onProjectReady(projectId); onClose(); }} />
        : progress && targetId
        ? <ConnectionProgress api={api} targetId={targetId} onEdit={() => setProgress(false)} onClose={close} onBrowse={onProjectReady ? () => setBrowsing(true) : undefined} />
        : <form className="target-form" onSubmit={submit}>
          {hosts.length > 0 && <label>{text("importConfig")}
            <select disabled={busy} value="" onChange={event => importHost(event.target.value)}>
              <option value="">{text("importPlaceholder")}</option>
              {hosts.map(host => <option key={host} value={host}>{host}</option>)}
            </select>
          </label>}
          <div className="target-row">
            <label className="target-grow">{text("host")}<input autoFocus required maxLength={256} disabled={busy} placeholder={text("hostPlaceholder")} value={destination} onChange={event => { setDestination(event.target.value); setResolved(""); }} /></label>
            <label className="target-port">{text("port")}<input inputMode="numeric" pattern="[0-9]*" maxLength={5} disabled={busy} placeholder="22" value={port} onChange={event => setPort(event.target.value)} /></label>
          </div>
          <label>{text("user")}<input maxLength={256} disabled={busy} placeholder={text("userPlaceholder")} value={user} onChange={event => setUser(event.target.value)} /></label>
          <div className="target-auth" role="radiogroup" aria-label={text("auth")}>
            <span>{text("auth")}</span>
            {(["system", "password", "private_key"] as const).map(option => <button key={option} type="button" role="radio" aria-checked={auth === option} className={auth === option ? "selected" : ""} disabled={busy} onClick={() => { setAuth(option); setSecret(""); }}>{text(`auth_${option}`)}</button>)}
          </div>
          {auth === "system" && <p className="target-hint">{text("authSystemHint")}</p>}
          {auth === "private_key" && <label>{text("keyFile")}<input required maxLength={1024} disabled={busy} placeholder="~/.ssh/id_ed25519" value={identityFile} onChange={event => setIdentityFile(event.target.value)} /></label>}
          {auth !== "system" && <label>{text(auth === "password" ? "password" : "passphrase")}
            <span className="target-secret">
              <input type={showSecret ? "text" : "password"} autoComplete="off" disabled={busy} placeholder={text(entry ? "secretKeepHint" : auth === "password" ? "passwordHint" : "passphraseHint")} value={secret} onChange={event => setSecret(event.target.value)} />
              <button type="button" aria-label={text(showSecret ? "hideSecret" : "showSecret")} onClick={() => setShowSecret(value => !value)}>{text(showSecret ? "hideSecret" : "showSecret")}</button>
            </span>
          </label>}
          <details className="target-advanced">
            <summary>{text("advanced")}</summary>
            <label>{text("name")}<input maxLength={120} disabled={busy} placeholder={destination || text("namePlaceholder")} value={name} onChange={event => setName(event.target.value)} /></label>
            <label>{text("installDirectory")}<input maxLength={1024} disabled={busy} placeholder="~/.dogcodes" value={installDirectory} onChange={event => setInstallDirectory(event.target.value)} /></label>
            <p className="target-hint">{text("installDirectoryHint")}</p>
          </details>
          {message && <div className="error" role="alert">{message}</div>}
          <footer>
            <button className="secondary" type="button" disabled={busy} onClick={close}>{t("common.cancel")}</button>
            <button className="send" disabled={busy || !destination.trim()}>{text(entry ? "saveAndConnect" : "addAndConnect")}</button>
          </footer>
        </form>}
    </div>
  </div>;
}

const PHASE_KEYS = { idle: "statusIdle", connecting: "statusConnecting", connected: "statusConnected", failed: "statusFailed", disconnected: "statusDisconnected" } as const;

export function ExecutionTargets({ api = tauriTargetApi, requestNew = 0, onProjectReady }: {
  api?: TargetApi; requestNew?: number; onProjectReady?: (projectId: string) => void | Promise<void>;
}) {
  const text = useTargetText();
  const [entries, setEntries] = useState<TargetEntry[] | null>(null);
  const [failure, setFailure] = useState("");
  const [dialog, setDialog] = useState<{ entry: TargetEntry | null; progress?: boolean; browsing?: boolean } | null>(null);
  const [removing, setRemoving] = useState<TargetEntry | null>(null);
  const [revision, setRevision] = useState(0);
  useEffect(() => { if (requestNew > 0) setDialog({ entry: null }); }, [requestNew]);
  useEffect(() => {
    let closed = false;
    const read = () => void api.list().then(value => {
      if (!closed) { setEntries(value); setFailure(""); }
    }).catch(cause => { if (!closed) setFailure(userFacingError(cause, text("loadFailed"))); });
    read();
    const timer = setInterval(read, 2000);
    return () => { closed = true; clearInterval(timer); };
  }, [api, revision]);
  const changed = () => setRevision(value => value + 1);
  const act = (work: Promise<unknown>) => void work.then(changed).catch(cause => setFailure(userFacingError(cause, text("actionFailed"))));
  return <section className="workspace remote-page target-page" aria-label={text("title")}>
    <div className="remote-page-content">
      <header className="remote-header">
        <span className="remote-heading-icon"><SidebarNavIcon kind="server" /></span>
        <p>{text("subtitle")}</p>
        <button className="send target-new" type="button" onClick={() => setDialog({ entry: null })}>{text("add")}</button>
      </header>
      {failure && <div className="remote-error" role="alert">{failure}</div>}
      {!entries && !failure && <LoadingStatus>{text("loading")}</LoadingStatus>}
      {entries?.length === 0 && <p className="remote-empty">{text("empty")}</p>}
      {!!entries?.length && <ul className="target-list">
        {entries.map(entry => {
          const { target, status } = entry;
          const live = status.phase === "connected" || status.phase === "connecting";
          return <li key={target.targetId} className="remote-device">
            <span className="remote-device-icon"><SidebarNavIcon kind="server" /></span>
            <div className="remote-device-identity">
              <strong>{target.displayName}</strong>
              <span>{targetRoute(target.connection)}</span>
              {status.error && <small className="target-row-error">{text(ERROR_KEYS[status.error.kind])}</small>}
            </div>
            <span className={`target-phase ${status.phase}`} role="status"><i aria-hidden="true" />{text(PHASE_KEYS[status.phase])}</span>
            <div className="target-row-actions">
              {status.phase === "connected" && onProjectReady && <button type="button" onClick={() => setDialog({ entry, progress: true, browsing: true })}>{text("openProject")}</button>}
              {live
                ? <button type="button" onClick={() => act(api.disconnect(target.targetId))}>{text("disconnect")}</button>
                : <button type="button" onClick={() => { setDialog({ entry, progress: true }); act(api.connect(target.targetId, null)); }}>{text("connect")}</button>}
              <button type="button" onClick={() => setDialog({ entry })}>{text("edit")}</button>
              <button type="button" onClick={() => setRemoving(entry)}>{text("remove")}</button>
            </div>
          </li>;
        })}
      </ul>}
    </div>
    {removing && <ConfirmDialog title={text("remove")} tone="danger" icon="trash" confirmLabel={text("remove")} onCancel={() => setRemoving(null)}
      onConfirm={() => { setRemoving(null); act(api.remove(removing.target.targetId)); }}>
      <p>{text("removeConfirm", { name: removing.target.displayName })}</p>
    </ConfirmDialog>}
    {dialog && <TargetConnectionDialog api={api} entry={dialog.entry} initialProgress={dialog.progress} initialBrowsing={dialog.browsing} onChanged={changed} onClose={() => setDialog(null)} onProjectReady={onProjectReady} />}
  </section>;
}
