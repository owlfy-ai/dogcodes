// Local Vite test entry only; not imported by the product or its production HTML entry.
import { useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import type { ArtifactId, TaskArtifactSummary, TaskId } from "@dogcodes/protocol";
import { ArtifactBrowser, type ArtifactReadClient } from "../src/ArtifactBrowser";
import "../src/styles.css";

const taskId: TaskId = `tsk_${"0".repeat(26)}`;
const encode = (value: string) => new TextEncoder().encode(value);
const contents = [
  encode("diff --git a/src/greet.ts b/src/greet.ts\nindex abc..def 100644\n--- a/src/greet.ts\n+++ b/src/greet.ts\n@@ -1,3 +1,4 @@\n export function greet() {\n-  return 'hello';\n+  // <img src=\"https://example.invalid/track\">\n+  return '你好';\n }\n" +
    "diff --git a/old name.md b/new name.md\nsimilarity index 100%\nrename from old name.md\nrename to new name.md\n" +
    "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n" +
    "diff --git a/image.bin b/image.bin\nindex abc..def 100644\nGIT binary patch\nliteral 2\nAcmePayload\n\n"),
  encode("diff --git a/incomplete.txt b/incomplete.txt\n@@ -1,2 +1,2 @@\n one\n"),
  encode(`diff --git a/large.txt b/large.txt\nnew file mode 100644\n--- /dev/null\n+++ b/large.txt\n@@ -0,0 +1,10001 @@\n${"+line\n".repeat(10001)}`),
  new Uint8Array([100, 105, 102, 102, 255]),
];

const items: TaskArtifactSummary[] = await Promise.all(contents.map(async (content, index) => {
  const digest = await crypto.subtle.digest("SHA-256", new Uint8Array(content));
  const artifactId: ArtifactId = `art_${"0".repeat(25)}${index + 1}`;
  return { metadata: { artifactId, mediaType: "application/vnd.dogcodes.git-diff.v1.patch",
    payloadSha256: [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, "0")).join(""),
    byteLength: content.length, createdAt: "2026-09-03T00:00:00Z", redactionRulesetVersion: 0, redactionCount: 0 },
    sourceSequence: 4 - index, sourceKind: "workspace.diff_captured" };
}));

function Harness() {
  const [open, setOpen] = useState(false);
  const [deferReads, setDeferReads] = useState(false);
  const client = useMemo<ArtifactReadClient>(() => ({
    taskArtifacts: async () => ({ items, nextCursor: null }),
    readArtifact: async (request) => {
      const index = items.findIndex((item) => item.metadata.artifactId === request.artifactId);
      const item = items[index]; const content = contents[index];
      if (request.taskId !== taskId || !item || !content || request.expectedPayloadSha256 !== item.metadata.payloadSha256) throw new Error("测试夹具请求不匹配");
      if (deferReads) await new Promise<void>((resolve) => setTimeout(resolve, 600));
      const end = Math.min(request.offset + request.limit, content.length);
      return { metadata: item.metadata, offset: request.offset, bytes: [...content.slice(request.offset, end)], nextOffset: end < content.length ? end : null };
    },
    exportArtifact: async () => { throw new Error("测试夹具不导出文件；原生保存必须单独验收"); },
  }), [deferReads]);
  return <main className="artifact-harness">
    <h1>Artifact Review 本地测试夹具</h1>
    <p>仅浏览器内存数据；不读取项目、数据库或磁盘文件，也不导出。第 1 项是多文件 Diff，第 2 项格式无效，第 3 项超展示行预算，第 4 项非 UTF-8。</p>
    <label><input type="checkbox" checked={deferReads} disabled={open} onChange={(event) => setDeferReads(event.target.checked)} />读取响应延迟 600 ms（用于快速切换/关闭回归）</label>
    <p><button onClick={() => setOpen(true)}>打开 Artifact 测试</button></p>
    {open && <ArtifactBrowser client={client} taskId={taskId} onClose={() => setOpen(false)} />}
  </main>;
}

createRoot(document.getElementById("root")!).render(<Harness />);
