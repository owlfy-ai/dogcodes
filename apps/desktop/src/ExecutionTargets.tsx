import { type FormEvent, type MouseEvent, type ReactNode, type RefObject, useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ConfirmDialog } from "./AppDialog";
import { HostAgentManagement } from "./HostAgentManagement";
import { HostTerminalTab } from "./HostTerminalTab";
import { orderAgents, useAgentPreferences } from "./agentPreferences";
import { AgentIcon, providerLabel } from "./agentPresentation";
import { LoadingStatus } from "./LoadingStatus";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { SidebarNavIcon } from "./SidebarNavIcon";
import { TabBar } from "./TabBar";
import { focusDialogTitle } from "./dialogFocus";
import { useI18n } from "./i18n";
import { showContextMenu, type ContextMenuEntry } from "./ContextMenu";
import { userFacingError } from "./userFacingError";
import {
  type ProxyChoice, type SshAuth, type SshProxyKind, type TargetApi, type TargetEntry, type TargetErrorKind, type TargetStatus,
  connectionFromForm, joinDirectory, parentDirectory, sameRoute, stepStates, tauriTargetApi, targetRoute, type TargetPrompt
} from "./executionTargetClient";
import { needsPerson, useTargetConnection } from "./targetConnection";
import { hostTerminalClient, hostTerminalScope, terminalEnded, type TerminalApi, type TerminalStatus } from "./terminalClient";
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
  protocol_mismatch: "errorProtocolMismatch", proxy_unavailable: "errorProxyUnavailable", failed: "errorFailed",
};

/** `ssh` asks in English whatever the interface's language. Only what a server itself asks,
 * such as a one-time code, has no wording of ours. */
function promptLabel(prompt: TargetPrompt, text: ReturnType<typeof useTargetText>): string {
  if (prompt.kind === "password") return text("promptPassword");
  if (prompt.kind !== "passphrase") return prompt.text;
  const key = /key '(.+?)'/.exec(prompt.text)?.[1];
  return key ? text("promptPassphraseFor", { key }) : text("promptPassphrase");
}

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

const AUTO_RETRY_MILLISECONDS = 15000;
const AUTO_RECONNECT_MILLISECONDS = 3000;
/** Failures that can pass by themselves. Anything else needs the person, so it isn't retried. */
const PASSING_ERRORS: readonly TargetErrorKind[] = ["host_unreachable", "timeout", "connection_closed"];

/** Reconnecting from where the person is. The dialog appears only when they are needed,
 * for a password, a host key or a failure, and only in the view they clicked. */
function useReconnect(api: TargetApi, targetId: string, automatic = false) {
  const text = useTargetText();
  const owner = useId();
  const { state, reconnect, dismiss } = useTargetConnection(api, targetId);
  const connecting = state.status?.phase === "connecting";
  const phase = state.status?.phase;
  const errorKind = state.status?.error?.kind;
  // Whoever works in a project on a machine needs it connected, and kept connected.
  useEffect(() => {
    if (!automatic) return;
    const delay = phase === "idle" ? 0 : phase === "disconnected" ? AUTO_RECONNECT_MILLISECONDS
      : phase === "failed" && errorKind && PASSING_ERRORS.includes(errorKind) ? AUTO_RETRY_MILLISECONDS : null;
    if (delay === null) return;
    const timer = setTimeout(() => void reconnect(owner, true), delay);
    return () => clearTimeout(timer);
  }, [automatic, phase, errorKind, targetId]);
  return {
    status: state.status,
    connect: () => reconnect(owner),
    offline: state.offline,
    failure: state.failure ? userFacingError(state.failure, text("loadFailed")) : phase === "failed" && errorKind ? text(ERROR_KEYS[errorKind]) : "",
    button: { disabled: connecting, label: text(connecting ? "reconnecting" : "reconnect"), onClick: () => void reconnect(owner) },
    dialog: needsPerson(state) && state.entry && state.asker === owner
      ? <TargetConnectionDialog api={api} entry={state.entry} initialProgress onClose={dismiss} onChanged={() => {}} />
      : null,
  };
}

/** Sending would fail anyway: say why and reconnect from here. */
export function TargetOfflineBanner({ api = tauriTargetApi, targetId, autoConnect = false }: {
  api?: TargetApi; targetId: string; autoConnect?: boolean;
}) {
  const text = useTargetText();
  const { offline, failure, button, dialog } = useReconnect(api, targetId, autoConnect);
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

function ConnectionProgress({ api, targetId, onEdit, onClose, onBrowse, onCancel, autoBrowse = false, waitForScan = false }: {
  api: TargetApi; targetId: string; onEdit: () => void; onClose: () => void; onBrowse?: () => void;
  /** Connected only to find its agents, so finishing before that would throw it away. */
  waitForScan?: boolean;
  /** Goes on to the folders once connected, unless there is something to read here first. */
  autoBrowse?: boolean;
  /** Where cancelling leads, when this is shown in a tab instead of the settings dialog. */
  onCancel?: () => void;
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
  const ready = autoBrowse && status?.phase === "connected" && !status.scanning && status.agents.length > 0 && !status.identityChanged;
  useEffect(() => { if (ready) onBrowse?.(); }, [ready]);
  if (!status) return failure ? <div className="error" role="alert">{failure}</div> : <LoadingStatus>{text("loading")}</LoadingStatus>;
  const connected = status.phase === "connected";
  const deploying = status.step?.step === "deploying" ? status.step : null;
  // Why it ended is in `ssh`'s or the system's words, so it goes with their other output.
  const log = [
    ...status.log,
    ...[status.error?.detail, status.closedReason].filter((line): line is string => !!line && !status.log.join("\n").includes(line)),
  ];
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
        <label>{promptLabel(prompt, text)}<input autoFocus type="password" autoComplete="off" value={secret} onChange={event => setSecret(event.target.value)} /></label>
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
    {status.phase === "disconnected" && <div className="error" role="alert">{text("connectionLost")}</div>}
    {failure && <div className="error" role="alert">{failure}</div>}
    {log.length > 0 && <details className="target-log">
      <summary>{text("log")}</summary>
      <pre>{log.join("\n")}</pre>
    </details>}
    <footer>
      {(status.phase === "failed" || status.phase === "disconnected" || onCancel && status.phase === "idle") && <>
        <button className="secondary" type="button" onClick={onEdit}>{text("backToEdit")}</button>
        <button className="send" type="button" onClick={() => void api.connect(targetId, null).catch(cause => setFailure(userFacingError(cause, text("loadFailed"))))}>{text("retry")}</button>
      </>}
      {status.phase === "connecting" && !prompt && <button className="secondary" type="button" onClick={() => { void api.disconnect(targetId).catch(() => {}); (onCancel ?? onEdit)(); }}>{text("cancelConnecting")}</button>}
      {connected && <button className={onBrowse ? "secondary" : "send"} type="button" disabled={waitForScan && status.scanning} onClick={onClose}>{text("done")}</button>}
      {connected && onBrowse && <button className="send" type="button" disabled={status.scanning} onClick={onBrowse}>{text("openProject")}</button>}
    </footer>
  </div>;
}

export function TargetConnectionDialog({ api, entry, initialProgress = false, initialBrowsing = false, forProject = false, onClose, onChanged, onProjectReady }: {
  api: TargetApi; entry: TargetEntry | null; initialProgress?: boolean; initialBrowsing?: boolean; onClose: () => void; onChanged: () => void;
  /** Connects and goes on to choosing a folder. Otherwise this only edits the host's settings. */
  forProject?: boolean;
  onProjectReady?: (projectId: string) => void | Promise<void>;
}) {
  const { t } = useI18n();
  const text = useTargetText();
  const saved = entry?.target.connection;
  // A new host connects right away, to find out which agents it has.
  const connects = forProject || !entry;
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
  const savedProxy = saved?.proxy;
  const [proxyMode, setProxyMode] = useState<ProxyChoice>(savedProxy?.mode ?? "none");
  const [proxyKind, setProxyKind] = useState<SshProxyKind>(savedProxy?.mode === "custom" ? savedProxy.kind : "socks5");
  const [proxyHost, setProxyHost] = useState(savedProxy?.mode === "custom" ? savedProxy.host : "");
  const [proxyPort, setProxyPort] = useState(savedProxy?.mode === "custom" ? String(savedProxy.port) : "");
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
      const connection = connectionFromForm({ destination, port, user, auth, identityFile, installDirectory, proxyMode, proxyKind, proxyHost, proxyPort });
      const id = await api.save(targetId, name.trim() || destination.trim(), connection, auth === "system" ? null : secret || null);
      setTargetId(id);
      setSecret("");
      onChanged();
      // Sessions on an open link keep the old route, so only a changed route ends them.
      if (saved && !sameRoute(saved, connection)) await api.disconnect(id);
      if (!connects) return onClose();
      await api.connect(id, null);
      setProgress(true);
    } catch (cause) {
      setMessage(userFacingError(cause, text("saveFailed")));
    } finally {
      setBusy(false);
    }
  };
  const madeProject = useRef(false);
  const close = () => {
    if (busy) return;
    // A connection opened only to look around isn't needed once that is over.
    if (connects && targetId && !madeProject.current) void api.release(targetId).catch(() => {});
    onChanged();
    onClose();
  };
  const connectingOnOpen = useRef(false);
  useEffect(() => {
    if (!forProject || !entry || initialBrowsing || connectingOnOpen.current) return;
    connectingOnOpen.current = true;
    void api.connect(entry.target.targetId, null).catch(cause => setMessage(userFacingError(cause, text("loadFailed"))));
  }, [api]);
  const title = browsing ? text("browseTitle") : progress ? name.trim() || destination.trim() : entry ? text("editHost") : text("newHost");
  const preview = destination.trim() ? targetRoute(connectionFromForm({ destination, port, user, auth, identityFile, installDirectory, proxyMode, proxyKind, proxyHost, proxyPort })) : "user@host:22";
  // Both host-page and new-conversation entry points use this same modal. Keep
  // it outside their layout/style scopes (notably .remote-page button).
  return createPortal(<div className="modal-backdrop" role="presentation" onClick={event => event.stopPropagation()} onMouseDown={event => { if (event.target === event.currentTarget) close(); }}>
    <div ref={dialog} className="project-dialog target-dialog" role="dialog" aria-modal="true" aria-labelledby="target-dialog-title"
      onKeyDown={event => { if (event.key === "Escape") { event.preventDefault(); close(); } }}>
      <header>
        <div><h2 id="target-dialog-title" tabIndex={-1} data-dialog-title>{title}</h2><small className="target-route">{resolved || preview}</small></div>
        <button type="button" aria-label={t("common.close")} disabled={busy} onClick={close}>×</button>
      </header>
      {browsing && targetId && onProjectReady
        ? <RemoteProjectDialog api={api} targetId={targetId} onClose={close} onReady={async projectId => { madeProject.current = true; await onProjectReady(projectId); onClose(); }} />
        : progress && targetId
        ? <ConnectionProgress api={api} targetId={targetId} onEdit={() => setProgress(false)} onClose={close} onBrowse={onProjectReady ? () => setBrowsing(true) : undefined} autoBrowse={forProject} waitForScan={!entry && !forProject} />
        : <form className="target-form" onSubmit={submit}>
          <label>{text("alias")}<input maxLength={120} disabled={busy} placeholder={destination || text("namePlaceholder")} value={name} onChange={event => setName(event.target.value)} /></label>
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
            <label>{text("installDirectory")}<input maxLength={1024} disabled={busy} placeholder="~/.dogcodes" value={installDirectory} onChange={event => setInstallDirectory(event.target.value)} /></label>
            <p className="target-hint">{text("installDirectoryHint")}</p>
            <label>{text("proxy")}<select disabled={busy} value={proxyMode} onChange={event => setProxyMode(event.target.value as ProxyChoice)}>
              <option value="none">{text("proxyNone")}</option>
              <option value="system">{text("proxySystem")}</option>
              <option value="custom">{text("proxyCustom")}</option>
            </select></label>
            {proxyMode === "custom" && <div className="target-proxy">
              <label>{text("proxyType")}<select disabled={busy} value={proxyKind} onChange={event => setProxyKind(event.target.value as SshProxyKind)}>
                <option value="socks5">SOCKS5</option>
                <option value="http">HTTP</option>
              </select></label>
              <label>{text("proxyHost")}<input maxLength={255} disabled={busy} autoCapitalize="off" spellCheck={false} placeholder="127.0.0.1" value={proxyHost} onChange={event => setProxyHost(event.target.value)} /></label>
              <label>{text("proxyPort")}<input inputMode="numeric" maxLength={5} disabled={busy} placeholder="7890" value={proxyPort} onChange={event => setProxyPort(event.target.value)} /></label>
            </div>}
            <p className="target-hint">{text("proxyHint")}</p>
          </details>
          {message && <div className="error" role="alert">{message}</div>}
          <footer>
            <button className="secondary" type="button" disabled={busy} onClick={close}>{t("common.cancel")}</button>
            <button className="send" disabled={busy || !destination.trim()}>{text(!connects ? "saveHost" : entry ? "saveAndConnect" : "addAndConnect")}</button>
          </footer>
        </form>}
    </div>
  </div>, document.body);
}

/** From a host to a project on it: connect if needed, then choose a folder. With no
 * host, its settings come first. */
export function HostProjectDialog({ api = tauriTargetApi, entry, onChanged = () => {}, onClose, onProjectReady }: {
  api?: TargetApi; entry: TargetEntry | null; onChanged?: () => void; onClose: () => void; onProjectReady?: (projectId: string) => void | Promise<void>;
}) {
  return <TargetConnectionDialog api={api} entry={entry} forProject initialProgress={Boolean(entry)} initialBrowsing={entry?.status.phase === "connected"} onChanged={onChanged} onClose={onClose} onProjectReady={onProjectReady} />;
}

const HOST_LIST = "hosts";

function ActionButton({ label, path, onClick }: { label: string; path: string; onClick: (event: MouseEvent) => void }) {
  return <button type="button" aria-label={label} data-tooltip={label} onClick={onClick}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={path} /></svg></button>;
}

/** The agents found on a host, shown whether or not it is connected. Refreshing a host
 * nothing else is using connects only for that long. */
function HostAgents({ api, entry, inUse, onAct, onChanged, onManage, refreshers }: {
  api: TargetApi; entry: TargetEntry; inUse: boolean; onAct: (work: Promise<unknown>) => void; onChanged: () => void; onManage: () => void;
  /** Lets the host's right-click menu do what this section's refresh button does. */
  refreshers: RefObject<Map<string, { refresh: () => void; busy: boolean }>>;
}) {
  const text = useTargetText();
  const { resolvedLanguage: lang } = useI18n();
  const preferences = useAgentPreferences();
  const { targetId } = entry.target;
  const { status: live, connect, dialog } = useReconnect(api, targetId);
  const [connectedToLook, setConnectedToLook] = useState(false);
  useEffect(() => {
    if (!connectedToLook) return;
    if (live?.phase === "failed" || live?.phase === "disconnected") setConnectedToLook(false);
    // The page's own reading: it follows closely while a scan runs. It wasn't connected
    // when the refresh was clicked, so this can only be the connection just made.
    if (entry.status.phase !== "connected" || entry.status.scanning) return;
    setConnectedToLook(false);
    if (!inUse) void api.release(targetId).catch(() => {}).finally(onChanged);
  }, [connectedToLook, live?.phase, entry.status.phase, entry.status.scanning, inUse]);
  const refresh = () => {
    if (entry.status.phase === "connected") return onAct(api.rescan(targetId));
    void connect().then(() => { setConnectedToLook(true); onChanged(); });
  };
  const busy = connectedToLook || entry.status.scanning || entry.status.phase === "connecting";
  const agents = orderAgents(entry.agents, agent => agent.providerId, preferences);
  refreshers.current.set(targetId, { refresh, busy });
  return <div className="target-card-agents">
    {agents.length > 0
      ? <ul aria-label={text("agentsTitle")}>{agents.map(agent => {
        const label = `${providerLabel(agent.providerId, lang)} · ${text(AGENT_STATUS_KEYS[agent.status])}`;
        return <li key={agent.providerId} role="img" aria-label={label} data-tooltip={label} className={agent.status === "ready" ? undefined : "target-card-agent-unready"}><AgentIcon providerId={agent.providerId} size="small" /></li>;
      })}</ul>
      : !busy && <span className="target-hint">{text("noAgentsOnHost")}</span>}
    {busy
      ? <LoadingStatus>{text("scanningAgents")}</LoadingStatus>
      : <div className="target-row-actions target-card-connect" onDoubleClick={event => event.stopPropagation()}>
        <ActionButton label={text("refreshAgents")} path="M20 11a8 8 0 0 0-14-4.5L4 9m0-5v5h5m-5 4a8 8 0 0 0 14 4.5l2-2.5m0 5v-5h-5" onClick={refresh} />
        <ActionButton label={text("manageAgents")} path="M4 7h10m4 0h2M4 17h2m4 0h10M14 5v4h4V5h-4ZM6 15v4h4v-4H6Z" onClick={onManage} />
      </div>}
    {dialog}
  </div>;
}

interface ClosableTab { id: string; running: boolean; close: () => Promise<void> }

interface HostTab {
  id: string;
  targetId: string;
  /** Tells apart several shells on one host. */
  ordinal: number;
  /** One shell. Reconnecting starts another in the same tab. */
  operationId: string;
  /** Run once in the new shell, for an install or a sign-in asked for elsewhere. */
  initialCommand?: string;
  resourceId?: string;
  status?: TerminalStatus;
  epoch?: string;
}

export function ExecutionTargets({ api = tauriTargetApi, terminals = hostTerminalClient, visible = true, onProjectReady }: {
  api?: TargetApi; terminals?: (targetId: string) => TerminalApi; visible?: boolean; onProjectReady?: (projectId: string) => void | Promise<void>;
}) {
  const text = useTargetText();
  const { t } = useI18n();
  const [entries, setEntries] = useState<TargetEntry[] | null>(null);
  const [failure, setFailure] = useState("");
  const [dialog, setDialog] = useState<{ entry: TargetEntry | null; forProject?: boolean } | null>(null);
  const [removing, setRemoving] = useState<TargetEntry | null>(null);
  const [revision, setRevision] = useState(0);
  const [tabs, setTabs] = useState<HostTab[]>([]);
  const [selected, setSelected] = useState(HOST_LIST);
  const [agentPages, setAgentPages] = useState<string[]>([]);
  const latestTabs = useRef(tabs);
  latestTabs.current = tabs;
  const refreshers = useRef(new Map<string, { refresh: () => void; busy: boolean }>());
  const [closingRunning, setClosingRunning] = useState<ClosableTab[] | null>(null);
  const restored = useRef(false);
  // A tab waiting for its connection should follow it closely.
  const waiting = tabs.some(tab => !tab.status) || !!entries?.some(({ status }) => status.scanning || status.phase === "connecting");
  useEffect(() => {
    if (!visible) return;
    let closed = false;
    const read = () => void api.list().then(value => {
      if (!closed) { setEntries(value); setFailure(""); }
    }).catch(cause => { if (!closed) setFailure(userFacingError(cause, text("loadFailed"))); });
    read();
    const timer = setInterval(read, waiting ? POLL_MILLISECONDS : 2000);
    return () => { closed = true; clearInterval(timer); };
  }, [api, revision, visible, waiting]);
  // Shells outlive this page and the window, so put back the tabs of those still there.
  // One that a restart took with it can't come back, and its output is gone too. A new
  // shell runs nothing by itself, so its tab simply carries on with one.
  useEffect(() => {
    if (!entries || restored.current) return;
    restored.current = true;
    for (const entry of entries) {
      const { targetId } = entry.target;
      const client = terminals(targetId);
      void client.list(hostTerminalScope(targetId)).then(shells => {
        const open = [...shells].reverse().filter(info => !terminalEnded(info));
        const lost = open.filter(info => info.status === "unknown");
        for (const info of lost) void client.close(info.id, info.epoch).catch(() => {});
        setTabs(current => {
          const next = [...current];
          for (const info of open) {
            if (next.some(tab => tab.operationId === info.id)) continue;
            const ordinal = Math.max(0, ...next.filter(tab => tab.targetId === targetId).map(tab => tab.ordinal)) + 1;
            next.push({ id: crypto.randomUUID(), targetId, ordinal, ...(info.status === "unknown"
              ? { operationId: crypto.randomUUID() }
              : { operationId: info.id, resourceId: info.id, status: info.status, epoch: info.epoch }) });
          }
          return next;
        });
        if (lost.length) connect(entry);
      }).catch(() => {});
    }
  }, [entries, terminals]);
  const changed = () => setRevision(value => value + 1);
  const act = (work: Promise<unknown>) => void work.then(changed).catch(cause => setFailure(userFacingError(cause, text("actionFailed"))));
  const connect = (entry: TargetEntry) => {
    if (entry.status.phase !== "connected" && entry.status.phase !== "connecting") act(api.connect(entry.target.targetId, null));
  };
  const openTerminal = (entry: TargetEntry, initialCommand?: string) => {
    const { targetId } = entry.target;
    const id = crypto.randomUUID();
    setTabs(current => [...current, { id, targetId, operationId: crypto.randomUUID(), initialCommand, ordinal: Math.max(0, ...current.filter(tab => tab.targetId === targetId).map(tab => tab.ordinal)) + 1 }]);
    setSelected(id);
    connect(entry);
  };
  const agentPageId = (targetId: string) => `agents:${targetId}`;
  const openAgentPage = (targetId: string) => {
    setAgentPages(current => current.includes(targetId) ? current : [...current, targetId]);
    setSelected(agentPageId(targetId));
  };
  /** The page asked for a connection, so it lets go of it unless a shell needs it. */
  const closeAgentPage = (targetId: string) => {
    setAgentPages(current => current.filter(id => id !== targetId));
    setSelected(current => current === agentPageId(targetId) ? HOST_LIST : current);
    if (!latestTabs.current.some(tab => tab.targetId === targetId)) act(api.release(targetId));
  };
  const updateTab = (id: string, change: Partial<HostTab>) => setTabs(current => current.map(tab => tab.id === id ? { ...tab, ...change } : tab));
  const forgetTab = (id: string) => {
    const open = latestTabs.current;
    const index = open.findIndex(tab => tab.id === id);
    // Now rather than at the next render: closing several checks what's left after each.
    latestTabs.current = open.filter(tab => tab.id !== id);
    setTabs(current => current.filter(tab => tab.id !== id));
    setSelected(current => current === id ? open[index + 1]?.id ?? open[index - 1]?.id ?? HOST_LIST : current);
  };
  /** Closing a tab ends only its own session. The service then ends the connection if
   * that was the last one on the host and no project there needs it for its agents. */
  const closeTab = async (tab: HostTab) => {
    try {
      // Nothing has started yet when there is no epoch, so there is nothing to stop.
      if (tab.epoch) await terminals(tab.targetId).close(tab.resourceId ?? tab.operationId, tab.epoch);
      forgetTab(tab.id);
      if (!latestTabs.current.some(other => other.id !== tab.id && other.targetId === tab.targetId)) await api.release(tab.targetId);
      changed();
    } catch (cause) {
      setFailure(userFacingError(cause, text("actionFailed")));
    }
  };
  const reconnectTab = (tab: HostTab, entry: TargetEntry) => {
    // Otherwise it would come back as a tab of its own after the next start.
    if (tab.status === "unknown" && tab.epoch) void terminals(tab.targetId).close(tab.resourceId ?? tab.operationId, tab.epoch).catch(() => {});
    updateTab(tab.id, { operationId: crypto.randomUUID(), initialCommand: undefined, resourceId: undefined, status: undefined, epoch: undefined });
    connect(entry);
  };
  const remove = async ({ target: { targetId } }: TargetEntry) => {
    const open = latestTabs.current.filter(tab => tab.targetId === targetId);
    await Promise.allSettled(open.filter(tab => tab.epoch).map(tab => terminals(targetId).close(tab.resourceId ?? tab.operationId, tab.epoch!)));
    open.forEach(tab => forgetTab(tab.id));
    await api.remove(targetId);
  };
  const entryOf = (targetId: string) => entries?.find(entry => entry.target.targetId === targetId);
  const tabLabel = (tab: HostTab) => (entryOf(tab.targetId)?.target.displayName ?? "") + (tab.ordinal > 1 ? ` · ${tab.ordinal}` : "");
  const tabState = (tab: HostTab) => {
    if (tab.status === "running") return "connected";
    if (tab.status && tab.status !== "starting") return "disconnected";
    const phase = entryOf(tab.targetId)?.status.phase;
    return phase === "failed" || phase === "disconnected" ? "disconnected" : "connecting";
  };
  const closableTabs: ClosableTab[] = [
    ...agentPages.filter(targetId => entryOf(targetId)).map(targetId => ({ id: agentPageId(targetId), running: false, close: async () => closeAgentPage(targetId) })),
    ...tabs.map(tab => ({ id: tab.id, running: tab.status === "running", close: () => closeTab(tab) })),
  ];
  const closeMany = async (list: ClosableTab[]) => { for (const tab of list) await tab.close(); };
  // Closing several at once is easy to do without looking at each, so ask if it stops anything.
  const requestClose = (list: ClosableTab[]) => {
    if (list.some(tab => tab.running)) setClosingRunning(list);
    else void closeMany(list);
  };
  const tabMenu = (id: string): ContextMenuEntry[] => {
    const index = closableTabs.findIndex(tab => tab.id === id);
    const others = closableTabs.filter(tab => tab.id !== id), right = closableTabs.slice(index + 1);
    return [
      { label: t("contextMenu.close"), onSelect: () => void closableTabs[index]?.close() },
      { label: t("contextMenu.closeOthers"), disabled: !others.length, onSelect: () => requestClose(others) },
      { label: t("contextMenu.closeRight"), disabled: !right.length, onSelect: () => requestClose(right) },
    ];
  };
  const hostMenu = (entry: TargetEntry): ContextMenuEntry[] => {
    const { targetId } = entry.target;
    const agents = refreshers.current.get(targetId);
    return [
      { label: text("connect"), onSelect: () => openTerminal(entry) },
      onProjectReady && { label: text("openProject"), onSelect: () => setDialog({ entry, forProject: true }) },
      "separator",
      { label: text("manageAgents"), onSelect: () => openAgentPage(targetId) },
      { label: text("refreshAgents"), disabled: !agents || agents.busy, onSelect: () => agents?.refresh() },
      "separator",
      { label: text("edit"), onSelect: () => setDialog({ entry }) },
      { label: `${text("remove")}…`, onSelect: () => setRemoving(entry) },
    ];
  };
  return <section className="workspace remote-page target-page" aria-label={text("title")} hidden={!visible}>
    {/* A lone "hosts" tab is just a stray button: show the bar once there is a second tab. */}
    {(agentPages.length > 0 || tabs.length > 0) && <TabBar className="target-tabs" label={text("title")} selected={selected} onSelect={setSelected} revealKey={visible} items={[
      { id: HOST_LIST, label: text("title") },
      ...agentPages.flatMap(targetId => {
        const name = entryOf(targetId)?.target.displayName;
        if (!name) return [];
        const label = text("agentsOf", { name });
        return [{ id: agentPageId(targetId), label, menu: () => tabMenu(agentPageId(targetId)), close: { label: text("closeTab", { name: label }), onClose: () => closeAgentPage(targetId) } }];
      }),
      ...tabs.map(tab => ({
        id: tab.id, label: tabLabel(tab), menu: () => tabMenu(tab.id),
        tooltip: entryOf(tab.targetId) && targetRoute(entryOf(tab.targetId)!.target.connection),
        icon: <i className={`target-tab-state ${tabState(tab)}`} aria-hidden="true" />,
        close: { label: text("closeTab", { name: tabLabel(tab) }), onClose: () => void closeTab(tab) },
      })),
    ]} />}
    <div className="target-host-list" role="tabpanel" hidden={selected !== HOST_LIST}>
      <div className="remote-page-content">
        <header className="remote-header">
          <span className="remote-heading-icon"><SidebarNavIcon kind="server" /></span>
          <p>{text("subtitle")}<small>{text("hostsHint")}</small></p>
          <button className="send target-new" type="button" onClick={() => setDialog({ entry: null })}>{text("newHost")}</button>
        </header>
        {failure && <div className="remote-error" role="alert">{failure}</div>}
        {!entries && !failure && <LoadingStatus>{text("loading")}</LoadingStatus>}
        {entries?.length === 0 && <p className="remote-empty">{text("empty")}</p>}
        {!!entries?.length && <ul className="target-cards">
          {entries.map(entry => {
            const { target } = entry;
            return <li key={target.targetId} className="target-card" onDoubleClick={() => openTerminal(entry)} onContextMenu={event => showContextMenu(event, hostMenu(entry))}>
              {/* A click from the keyboard opens it too; a mouse needs the double click. */}
              <button type="button" className="target-card-open" aria-label={text("openTerminal", { name: target.displayName })} onClick={event => { if (event.detail === 0) openTerminal(entry); }}>
                <span className="remote-device-icon"><SidebarNavIcon kind="server" /></span>
                <span className="target-card-identity">
                  <strong>{target.displayName}</strong>
                  <span>{targetRoute(target.connection)}</span>
                </span>
              </button>
              {/* Hover actions float beside the terminal button instead of reserving room the name needs. */}
              <div className="target-card-end">
                <div className="target-row-actions" onDoubleClick={event => event.stopPropagation()}>
                    {onProjectReady && <ActionButton label={text("openProject")} path="M3 19V6a2 2 0 0 1 2-2h4l2 3h7a2 2 0 0 1 2 2v1M3 19l3-8a1 1 0 0 1 1-1h14l-3 8a1.5 1.5 0 0 1-1.4 1H3Z" onClick={() => setDialog({ entry, forProject: true })} />}
                    <ActionButton label={text("edit")} path="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16v4Zm9.5-13.5 4 4" onClick={() => setDialog({ entry })} />
                    <ActionButton label={text("remove")} path="M4 7h16M9 7V4h6v3m3 0-1 13H7L6 7m4 4v5m4-5v5" onClick={() => setRemoving(entry)} />
                </div>
                {/* The same as double-clicking the card. Always shown: it's how a host is used. */}
                <button type="button" className="target-card-terminal" aria-label={text("openTerminal", { name: target.displayName })} onDoubleClick={event => event.stopPropagation()}
                  onClick={event => { if (event.detail <= 1) openTerminal(entry); }}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="3" /><path d="m7 9 3 3-3 3m5 0h5" /></svg>
                  {text("terminalButton")}
                </button>
              </div>
              <HostAgents api={api} entry={entry} inUse={tabs.some(tab => tab.targetId === target.targetId)} onAct={act} onChanged={changed} onManage={() => openAgentPage(target.targetId)} refreshers={refreshers} />
            </li>;
          })}
        </ul>}
      </div>
    </div>
    {agentPages.map(targetId => {
      const entry = entryOf(targetId);
      return entry && <HostAgentManagement key={targetId} api={api} entry={entry} visible={visible && selected === agentPageId(targetId)} offlineBanner={<TargetOfflineBanner api={api} targetId={targetId} autoConnect />} onRunInShell={command => openTerminal(entry, command)} />;
    })}
    {tabs.map(tab => {
      const entry = entryOf(tab.targetId);
      return entry && <HostTerminalTab key={tab.id} targetId={tab.targetId} api={terminals(tab.targetId)} operationId={tab.operationId} resourceId={tab.resourceId} initialCommand={tab.initialCommand}
        status={tab.status} connected={entry.status.phase === "connected"} visible={visible && selected === tab.id}
        onStatus={(status, epoch) => { if (tab.status !== status || tab.epoch !== epoch) updateTab(tab.id, { status, epoch }); }}
        onReconnect={() => reconnectTab(tab, entry)}
        connecting={<div className="project-dialog target-dialog target-tab-progress">
          <header><div><h2>{entry.target.displayName}</h2><small className="target-route">{targetRoute(entry.target.connection)}</small></div></header>
          <ConnectionProgress api={api} targetId={tab.targetId} onEdit={() => setDialog({ entry })} onClose={() => {}} onCancel={() => forgetTab(tab.id)} />
        </div>} />;
    })}
    {closingRunning && <ConfirmDialog title={t("contextMenu.closeRunningTitle")} tone="danger" icon="trash" confirmLabel={t("contextMenu.close")}
      onCancel={() => setClosingRunning(null)} onConfirm={() => { const list = closingRunning; setClosingRunning(null); void closeMany(list); }}>
      <p>{t("contextMenu.closeRunningBody", { count: closingRunning.filter(tab => tab.running).length })}</p>
    </ConfirmDialog>}
    {removing && <ConfirmDialog title={text("remove")} tone="danger" icon="trash" confirmLabel={text("remove")} onCancel={() => setRemoving(null)}
      onConfirm={() => { setRemoving(null); act(remove(removing)); }}>
      <p>{text("removeConfirm", { name: removing.target.displayName })}</p>
    </ConfirmDialog>}
    {visible && dialog && (dialog.forProject && dialog.entry
      ? <HostProjectDialog api={api} entry={dialog.entry} onChanged={changed} onClose={() => setDialog(null)} onProjectReady={onProjectReady} />
      : <TargetConnectionDialog api={api} entry={dialog.entry} onChanged={changed} onClose={() => setDialog(null)} />)}
  </section>;
}
