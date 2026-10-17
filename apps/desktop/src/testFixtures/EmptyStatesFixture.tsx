// Isolated acceptance UI: mock IPC and terminal APIs never access local projects.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { CoreClient, HistoryPage, HistoryQuery, TaskId } from "@dogcodes/protocol";
import { WorkspaceFiles } from "../WorkspaceFiles";
import { WorkspaceFilePreview } from "../WorkspaceFilePreview";
import { HistoryBrowser } from "../HistoryBrowser";
import { TerminalDock, type TerminalApi } from "../TerminalDock";
import { Tooltips } from "../Tooltips";
import "../styles.css";
import "../conversation.css";
import "../review.css";

document.documentElement.dataset.theme = "light";
let holdReads = false;
const pendingReads = new Set<() => void>();
const waitForRead = () => holdReads ? new Promise<void>(resolve => pendingReads.add(resolve)) : Promise.resolve();
mockIPC(async (command, args) => {
  if (["core_workspace_roots", "core_workspace_find_files", "core_workspace_directory", "core_workspace_preview"].includes(command)) await waitForRead();
  if (command === "core_workspace_roots") return args && "taskId" in args && args.taskId === "no-directory" ? [] : ["/fixture/project"];
  if (command === "core_workspace_find_files") return { files: [], partial: false };
  if (command === "core_workspace_directory") {
    const path = args && "path" in args ? String(args.path) : "";
    return path ? [{ name: "index.ts", path: `${path}/index.ts`, directory: false }] : [{ name: "src", path: "src", directory: true }, { name: "docs", path: "docs", directory: true }, { name: "README.md", path: "README.md", directory: false }];
  }
  if (command === "core_workspace_preview") return { text: "", image: null };
  return null;
}, { shouldMockEvents: true });
const historyClient = { queryHistory: async (query: HistoryQuery): Promise<HistoryPage> => { await waitForRead(); return ({ items: [], nextCursor: query.text === "next" && !query.afterSequence ? 1000 : null, throughSequence: 42, coverage: "fixture" }); } } as unknown as CoreClient;
const terminalApi: TerminalApi = { list: async () => [], get: async () => { throw new Error("验收终端不存在"); }, attach: async () => { throw new Error("验收终端不存在"); }, open: async () => { await waitForRead(); return { id: crypto.randomUUID(), detach() {} }; }, write: async () => {}, resize: async () => {}, close: async () => {} };
const scenarios = { files: "文件搜索", "no-directory": "文件树无目录", terminal: "终端（关闭全部标签）", "terminal-no-directory": "终端无目录", history: "历史及搜索", markdown: "空 Markdown" };
function Fixture() {
  const [scenario, setScenario] = useState<keyof typeof scenarios>("no-directory");
  const [returned, setReturned] = useState(false);
  const [editing, setEditing] = useState(false);
  const [holding, setHolding] = useState(false);
  const terminal = scenario.startsWith("terminal");
  return <><div style={{ display: "flex", flexWrap: "wrap", gap: 10, padding: 10, fontSize: 12 }}>
    <select aria-label="验收场景" value={scenario} onChange={event => { setScenario(event.target.value as keyof typeof scenarios); setReturned(false); setEditing(false); }}>{Object.entries(scenarios).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select>
    <button onClick={() => { document.documentElement.dataset.theme = document.documentElement.dataset.theme === "light" ? "dark" : "light"; }}>切换主题</button>
    <label><input type="checkbox" checked={holding} onChange={event => { holdReads = event.target.checked; setHolding(holdReads); if (!holdReads) { pendingReads.forEach(resolve => resolve()); pendingReads.clear(); } }} />保持加载状态</label>
    {returned && <span>已返回对话</span>}
    {editing && <span role="status">已打开项目编辑（模拟）</span>}
  </div><div style={{ display: "flex", flexDirection: "column", width: terminal ? "min(760px, 100%)" : scenario === "history" ? "min(680px, 100%)" : "min(420px, 100%)", height: "calc(100vh - 54px)", margin: "0 auto" }}>
    {terminal ? <TerminalDock key={scenario} projectId="fixture" directories={scenario === "terminal" ? ["/fixture/web", "/fixture/server"] : []} open onClose={() => {}} api={terminalApi} onEditProject={() => setEditing(true)} />
      : scenario === "history" ? <HistoryBrowser client={historyClient} taskId={"fixture" as TaskId} onBack={() => setReturned(true)} />
      : <aside className="review-panel" style={{ position: "relative", inset: "auto", width: "100%", height: "100%", boxShadow: "none" }}>
        {scenario === "markdown" ? <><header className="review-file-heading"><strong>README.md</strong></header><WorkspaceFilePreview path="README.md" preview={{ text: "", image: null }} /></>
          : <WorkspaceFiles key={scenario} taskId={scenario} onEditProject={() => setEditing(true)} />}
      </aside>}
  </div><Tooltips /></>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
