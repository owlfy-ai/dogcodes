// Isolated visual fixture: never invokes native commands or opens a connection.
import { useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { ExecutionTargets, TargetConnectionDialog } from "../ExecutionTargets";
import { ProjectPicker } from "../ProjectPicker";
import { Tooltips } from "../Tooltips";
import type { TargetApi, TargetEntry, TargetStatus } from "../executionTargetClient";
import { terminalClient, type TerminalApi, type TerminalInfo } from "../terminalClient";
import "../styles.css";
import "../remoteDevices.css";

const idle: TargetStatus = { scanning: false, agents: [], phase: "idle", step: null, log: [], prompt: null, info: null, error: null, closedReason: null, identityChanged: false };
const agents: TargetStatus["agents"] = [
  { providerId: "kimi", displayName: "Kimi Code", executable: "/root/.kimi-code/bin/kimi", version: "2.5.0", status: "ready", diagnostic: null },
  { providerId: "claude-code", displayName: "Claude Code", executable: "/usr/local/bin/claude", version: "2.1.0", status: "not_configured", diagnostic: "not_configured" },
];
const info = { protocol: 1, version: "0.1.1", os: "linux", arch: "x86_64", home: "/home/dev", shell: "/bin/bash", machineIdentity: "fixture" };
const fingerprint = "The authenticity of host 'build.example.com (10.0.0.8)' can't be established.\nED25519 key fingerprint is SHA256:fixtureFingerprintOnly0000000000000000000000.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
const scenarios: Record<string, TargetStatus> = {
  connecting: { ...idle, phase: "connecting", step: { step: "connecting" } },
  hostKey: { ...idle, phase: "connecting", step: { step: "connecting" }, prompt: { id: "p", kind: "host_key", text: fingerprint } },
  password: { ...idle, phase: "connecting", step: { step: "connecting" }, prompt: { id: "p", kind: "password", text: "dev@build.example.com's password:" } },
  deploying: { ...idle, phase: "connecting", step: { step: "deploying", sent: 1_800_000, total: 4_000_000 }, log: ["platform: Linux x86_64"] },
  scanning: { ...idle, phase: "connected", step: { step: "ready" }, info, scanning: true },
  connected: { ...idle, phase: "connected", step: { step: "ready" }, info, agents, log: ["platform: Linux x86_64", "shim: uploaded"] },
  noAgents: { ...idle, phase: "connected", step: { step: "ready" }, info },
  changedIdentity: { ...idle, phase: "connected", step: { step: "ready" }, info, identityChanged: true },
  authFailed: { ...idle, phase: "failed", step: { step: "connecting" }, error: { kind: "auth_failed", detail: "dev@build.example.com: Permission denied (publickey,password)." }, log: ["dev@build.example.com: Permission denied (publickey,password)."] },
  hostKeyChanged: { ...idle, phase: "failed", step: { step: "connecting" }, error: { kind: "host_key_changed", detail: "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!" } },
  lost: { ...idle, phase: "disconnected", closedReason: "connection closed" },
};

function entries(status: TargetStatus): TargetEntry[] {
  return [
    { target: { targetId: "tgt_one", displayName: "Build server", connection: { destination: "build.example.com", user: "dev", port: 2222, auth: "password" } }, status },
    { target: { targetId: "tgt_two", displayName: "gpu-box", connection: { destination: "gpu-box", auth: "system" } }, status: idle },
  ];
}

const shell = (id: string, epoch = "fixture"): TerminalInfo => ({ id, epoch, projectId: "target:tgt_one", directoryIndex: 0, taskId: null, commandId: null, label: "terminal", cwd: "/home/dev", command: null, status: "running", exitCode: null, error: null, outputEnd: 0, createdAt: 0 });
const banner = "Welcome to Ubuntu 24.04 LTS\r\n\r\nLast login: Wed Nov  4 21:14:02 2026\r\n\x1b[32mdev@build\x1b[0m:\x1b[34m~\x1b[0m$ ";
const fixtureTerminals: TerminalApi = {
  ...terminalClient,
  list: async () => [],
  get: async id => shell(id),
  resize: async () => {},
  close: async () => {},
  input: async () => ({ state: "ready", revision: "1" }),
  open: async (_project, _index, _cols, _rows, receive, id) => {
    receive({ kind: "state", info: shell(id) });
    receive({ kind: "data", bytes: [...new TextEncoder().encode(banner)] });
    return { id, detach: () => {} };
  },
};

function Fixture() {
  const [scenario, setScenario] = useState("connecting");
  const [dialog, setDialog] = useState<"page" | "picker" | "form" | "progress" | "browse">(new URLSearchParams(location.search).has("page") ? "page" : "progress");
  const status = scenarios[scenario]!;
  const api = useMemo<TargetApi>(() => ({
    list: async () => entries(status),
    status: async () => status,
    save: async () => "tgt_one",
    remove: async () => {},
    connect: async () => {},
    disconnect: async () => {},
    release: async () => {},
    answer: async () => {},
    configHosts: async () => ["build", "gpu-box", "staging"],
    resolveHost: async alias => ({ host: `${alias}.example.com`, user: "dev", port: 22 }),
    browse: async (_, path) => ({ path: path ?? "/root", directories: ["app", "dogcodes", "notes"] }),
    createProject: async () => "prj_fixture",
  }), [status]);
  const entry = entries(status)[0]!;
  return <>
    <nav style={{ position: "fixed", zIndex: 1000, bottom: 8, left: 8, display: "flex", flexWrap: "wrap", gap: 6, maxWidth: "90vw" }} aria-label="Scenario">
      {(["page", "picker", "form", "progress", "browse"] as const).map(name => <button key={name} aria-pressed={dialog === name} onClick={() => setDialog(name)}>{name}</button>)}
      {Object.keys(scenarios).map(name => <button key={name} aria-pressed={scenario === name} onClick={() => setScenario(name)}>{name}</button>)}
    </nav>
    {dialog === "page" && <main className="conversation-column" style={{ height: "100vh" }}><ExecutionTargets api={api} terminals={() => fixtureTerminals} onProjectReady={() => {}} /></main>}
    {dialog === "picker" && <main style={{ padding: "200px 40px" }}><ProjectPicker projects={[]} selectedProject={null} disabled={false} onSelect={() => {}} onOpenFolder={() => {}} onChooseHost={() => {}} onNewHost={() => {}} targetApi={api} /></main>}
    {dialog === "form" && <TargetConnectionDialog key="form" api={api} entry={null} onClose={() => {}} onChanged={() => {}} />}
    {dialog === "progress" && <TargetConnectionDialog key={scenario} api={api} entry={entry} initialProgress onClose={() => {}} onChanged={() => {}} onProjectReady={() => {}} />}
    {dialog === "browse" && <TargetConnectionDialog key="browse" api={api} entry={entry} initialProgress initialBrowsing onClose={() => {}} onChanged={() => {}} onProjectReady={() => {}} />}
  </>;
}

createRoot(document.getElementById("root")!).render(<><Tooltips /><Fixture /></>);
