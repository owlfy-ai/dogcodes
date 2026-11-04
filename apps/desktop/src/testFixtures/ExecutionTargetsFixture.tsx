// Isolated visual fixture: never invokes native commands or opens a connection.
import { useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { ExecutionTargets, TargetConnectionDialog } from "../ExecutionTargets";
import { Tooltips } from "../Tooltips";
import type { TargetApi, TargetEntry, TargetStatus } from "../executionTargetClient";
import "../styles.css";
import "../remoteDevices.css";

const idle: TargetStatus = { phase: "idle", step: null, log: [], prompt: null, info: null, error: null, closedReason: null, identityChanged: false };
const info = { protocol: 1, version: "0.1.1", os: "linux", arch: "x86_64", home: "/home/dev", shell: "/bin/bash", machineIdentity: "fixture" };
const fingerprint = "The authenticity of host 'build.example.com (10.0.0.8)' can't be established.\nED25519 key fingerprint is SHA256:fixtureFingerprintOnly0000000000000000000000.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
const scenarios: Record<string, TargetStatus> = {
  connecting: { ...idle, phase: "connecting", step: { step: "connecting" } },
  hostKey: { ...idle, phase: "connecting", step: { step: "connecting" }, prompt: { id: "p", kind: "host_key", text: fingerprint } },
  password: { ...idle, phase: "connecting", step: { step: "connecting" }, prompt: { id: "p", kind: "password", text: "dev@build.example.com's password:" } },
  deploying: { ...idle, phase: "connecting", step: { step: "deploying", sent: 1_800_000, total: 4_000_000 }, log: ["platform: Linux x86_64"] },
  connected: { ...idle, phase: "connected", step: { step: "ready" }, info, log: ["platform: Linux x86_64", "shim: uploaded"] },
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

function Fixture() {
  const [scenario, setScenario] = useState("connecting");
  const [dialog, setDialog] = useState<"page" | "form" | "progress">("progress");
  const status = scenarios[scenario]!;
  const api = useMemo<TargetApi>(() => ({
    list: async () => entries(status),
    status: async () => status,
    save: async () => "tgt_one",
    remove: async () => {},
    connect: async () => {},
    disconnect: async () => {},
    answer: async () => {},
    configHosts: async () => ["build", "gpu-box", "staging"],
    resolveHost: async alias => ({ host: `${alias}.example.com`, user: "dev", port: 22 }),
  }), [status]);
  const entry = entries(status)[0]!;
  return <>
    <nav style={{ position: "fixed", zIndex: 1000, top: 8, left: 8, display: "flex", flexWrap: "wrap", gap: 6, maxWidth: "90vw" }} aria-label="Scenario">
      {(["page", "form", "progress"] as const).map(name => <button key={name} aria-pressed={dialog === name} onClick={() => setDialog(name)}>{name}</button>)}
      {Object.keys(scenarios).map(name => <button key={name} aria-pressed={scenario === name} onClick={() => setScenario(name)}>{name}</button>)}
    </nav>
    {dialog === "page" && <main className="conversation-column" style={{ height: "100vh" }}><ExecutionTargets api={api} /></main>}
    {dialog === "form" && <TargetConnectionDialog key="form" api={api} entry={null} onClose={() => {}} onChanged={() => {}} />}
    {dialog === "progress" && <TargetConnectionDialog key={scenario} api={api} entry={entry} initialProgress onClose={() => {}} onChanged={() => {}} />}
  </>;
}

createRoot(document.getElementById("root")!).render(<><Tooltips /><Fixture /></>);
