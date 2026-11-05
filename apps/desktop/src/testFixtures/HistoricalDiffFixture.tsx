// Manual browser acceptance only; no workspace reads or mutations.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import type { ArtifactMetadata, CoreClient } from "@dogcodes/protocol";
import { HistoricalTurnDiff } from "../HistoricalTurnDiff";
import { Tooltips } from "../Tooltips";
import "../styles.css";
import "../conversation.css";

const query = new URLSearchParams(location.search);
document.documentElement.dataset.theme = query.get("theme") ?? "light";
const patch = 'diff --git a/src/greeting.ts b/src/greeting.ts\nindex abc..def 100644\n--- a/src/greeting.ts\n+++ b/src/greeting.ts\n@@ -10,4 +10,5 @@\n export function greet(name: string) {\n-  const message = "Hi " + name;\n+  const message = `Hello, ${name}!`;\n+  console.log(message);\n   return message;\n }\n@@ -100 +101 @@\n-export const ready = false;\n+export const ready = true;\n' +
  'diff --git a/hello.txt b/hello.txt\nnew file mode 100644\n--- /dev/null\n+++ b/hello.txt\n@@ -0,0 +1 @@\n+hello\n' +
  'diff --git a/logo.bin b/logo.bin\nBinary files a/logo.bin and b/logo.bin differ\n';
const bytes = new TextEncoder().encode(JSON.stringify({ files: [{ path: "src/greeting.ts", status: "modified" }, { path: "hello.txt", status: "added" }, { path: "logo.bin", status: "modified" }], patch }));
const hash = [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map(value => value.toString(16).padStart(2, "0")).join("");
const metadata: ArtifactMetadata = { artifactId: "art_historical_fixture", payloadSha256: hash, byteLength: bytes.length, mediaType: "application/json", createdAt: "2026-11-04T16:00:00Z", redactionRulesetVersion: 1, redactionCount: 0 };
const client = { readArtifact: async () => ({ metadata, offset: 0, bytes: [...bytes], nextOffset: null }) } as unknown as CoreClient;
function Fixture() {
  const [open, setOpen] = useState(true);
  return <main style={{ padding: 48 }}><button onClick={() => setOpen(true)}>Open historical diff</button>
    {open && <HistoricalTurnDiff client={client} taskId="tsk_fixture" metadata={metadata} initialPath={query.get("path") ?? "src/greeting.ts"} onClose={() => setOpen(false)} />}
  </main>;
}
createRoot(document.getElementById("root")!).render(<><Fixture /><Tooltips /></>);
