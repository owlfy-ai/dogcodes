import { invoke } from "@tauri-apps/api/core";

export type SshAuth = "system" | "password" | "private_key";
export interface SshConnection {
  destination: string;
  port?: number;
  user?: string;
  auth: SshAuth;
  identityFile?: string;
  installDirectory?: string;
}
export interface ExecutionTarget {
  targetId: string;
  displayName: string;
  connection: SshConnection;
  machineIdentity?: string;
}
export type TargetPhase = "idle" | "connecting" | "connected" | "failed" | "disconnected";
export type ConnectStep =
  | { step: "connecting" | "inspecting" | "starting" | "ready" }
  | { step: "deploying"; sent: number; total: number };
export interface TargetPrompt { id: string; kind: "password" | "passphrase" | "host_key" | "other"; text: string }
export interface TargetInfo { protocol: number; version: string; os: string; arch: string; home: string; shell: string | null; machineIdentity: string }
export type TargetErrorKind =
  | "ssh_missing" | "ssh_too_old" | "auth_failed" | "host_key_changed" | "host_key_rejected"
  | "host_unreachable" | "connection_closed" | "timeout" | "cancelled" | "account_restricted" | "unsupported_platform"
  | "shim_unavailable" | "home_missing" | "directory_unwritable" | "unsafe_directory"
  | "upload_failed" | "not_executable" | "bad_binary" | "protocol_mismatch" | "failed";
export interface TargetAgent {
  providerId: string;
  displayName: string;
  executable: string;
  version: string | null;
  status: "installed" | "not_configured" | "ready" | "unsupported_version" | "broken";
  diagnostic: string | null;
}
export interface TargetStatus {
  scanning: boolean;
  agents: TargetAgent[];
  phase: TargetPhase;
  step: ConnectStep | null;
  log: string[];
  prompt: TargetPrompt | null;
  info: TargetInfo | null;
  error: { kind: TargetErrorKind; detail: string } | null;
  closedReason: string | null;
  identityChanged: boolean;
}
export interface TargetEntry { target: ExecutionTarget; status: TargetStatus }
export interface TargetDirectory { path: string; directories: string[] }
export interface ResolvedHost { host: string; user: string | null; port: number | null }

/** The page takes this as a value so its visual fixture needs no native process. */
export interface TargetApi {
  list(): Promise<TargetEntry[]>;
  status(targetId: string): Promise<TargetStatus>;
  save(targetId: string | null, displayName: string, connection: SshConnection): Promise<string>;
  remove(targetId: string): Promise<void>;
  connect(targetId: string, secret: string | null): Promise<void>;
  disconnect(targetId: string): Promise<void>;
  answer(targetId: string, promptId: string, answer: string | null, remember: boolean): Promise<void>;
  configHosts(): Promise<string[]>;
  resolveHost(alias: string): Promise<ResolvedHost | null>;
  browse(targetId: string, path: string | null): Promise<TargetDirectory>;
  createProject(targetId: string, path: string, displayName: string): Promise<string>;
}

const call = <T,>(request: Record<string, unknown>) => invoke<T>("core_execution_target", { call: request });

export const tauriTargetApi: TargetApi = {
  list: () => call<TargetEntry[]>({ type: "list" }),
  status: targetId => call<TargetStatus>({ type: "status", targetId }),
  save: (targetId, displayName, connection) => call<string>({ type: "save", targetId, displayName, connection }),
  remove: targetId => call<void>({ type: "remove", targetId }),
  connect: (targetId, secret) => call<void>({ type: "connect", targetId, secret }),
  disconnect: targetId => call<void>({ type: "disconnect", targetId }),
  answer: (targetId, promptId, answer, remember) => call<void>({ type: "answer", targetId, promptId, answer, remember }),
  configHosts: () => call<string[]>({ type: "config_hosts" }),
  resolveHost: alias => call<ResolvedHost | null>({ type: "resolve_host", alias }),
  browse: (targetId, path) => call<TargetDirectory>({ type: "browse", targetId, path }),
  createProject: (targetId, path, displayName) => call<string>({ type: "create_project", targetId, path, displayName }),
};

export function targetRoute(connection: SshConnection): string {
  const user = connection.user ? `${connection.user}@` : "";
  const port = connection.port ? `:${connection.port}` : "";
  return `${user}${connection.destination}${port}`;
}

/** The parent of a POSIX path, whichever system this app runs on. */
export function parentDirectory(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const slash = trimmed.lastIndexOf("/");
  return slash <= 0 ? "/" : trimmed.slice(0, slash);
}

export function joinDirectory(path: string, name: string): string {
  return `${path.replace(/\/+$/, "")}/${name}`;
}

/** Empty optional fields are left out so `ssh` resolves them from its own config. */
export function connectionFromForm(form: {
  destination: string; port: string; user: string; auth: SshAuth; identityFile: string; installDirectory: string;
}): SshConnection {
  const port = Number.parseInt(form.port.trim(), 10);
  const user = form.user.trim();
  const installDirectory = form.installDirectory.trim();
  const identityFile = form.identityFile.trim();
  return {
    destination: form.destination.trim(),
    auth: form.auth,
    ...(Number.isInteger(port) ? { port } : {}),
    ...(user ? { user } : {}),
    ...(form.auth === "private_key" && identityFile ? { identityFile } : {}),
    ...(installDirectory ? { installDirectory } : {}),
  };
}

const STEP_ORDER = ["connecting", "inspecting", "deploying", "starting"] as const;
export type StepName = typeof STEP_ORDER[number];
export type StepState = "done" | "active" | "failed" | "pending";

/** Deploying is skipped when the helper is already installed, so "starting" implies it. */
export function stepStates(status: Pick<TargetStatus, "phase" | "step">): { name: StepName; state: StepState }[] {
  if (status.phase === "connected") return STEP_ORDER.map(name => ({ name, state: "done" }));
  const current = status.step ? STEP_ORDER.indexOf(status.step.step as StepName) : -1;
  return STEP_ORDER.map((name, index) => ({
    name,
    state: index < current ? "done" : index > current ? "pending" : status.phase === "failed" ? "failed" : "active",
  }));
}
