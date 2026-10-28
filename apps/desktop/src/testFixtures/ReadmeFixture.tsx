// Real product components with synthetic, read-only data for documentation.
// This entry is never imported by the desktop's production main.tsx.
import { createRoot } from "react-dom/client";
import type { BootstrapSnapshot, ExecutorRef, ModelCatalog, TaskId, TimelineItem, TurnReviewRun } from "@dogcodes/protocol";
import { App } from "../App";
import { DemoCoreClient } from "../demoClient";
import { Tooltips } from "../Tooltips";
import { projectRunClient, type ProjectRunCatalog, type RunProcess } from "../projectRunClient";
import { terminalClient, type TerminalInfo, type TerminalOutput } from "../terminalClient";
import type { ReviewSnapshot } from "../reviewClient";
import "../styles.css";

if ("__TAURI_INTERNALS__" in window) throw new Error("文档演示只能在隔离浏览器中打开，不能覆盖桌面连接");

const instant = "2026-10-17T09:30:00.000Z";
const projectId = "prj_01K00000000000000000000001" as const;
const taskId = "tsk_01K00000000000000000000001" as const;
const providers = ["codex", "claude-code", "cursor", "kimi", "grok", "zcode", "antigravity", "codebuddy", "pi", "opencode", "deepseek-harness", "kiro"];
const executors: ExecutorRef[] = providers.map((providerId, index) => ({ providerId, installationId: `ins_${String(index + 1).padStart(26, "0")}` }));
const catalog: ModelCatalog = { defaultModel: "gpt-5.4", models: [
  { id: "gpt-5.4", label: "GPT-5.4", thinkingOptions: [{ id: "high", label: "高" }, { id: "medium", label: "中" }], defaultThinking: "high" },
] };
const paths = ["src/components/SearchBox.tsx", "src/styles/search.css", "tests/search.test.tsx", "README.md"];
const before = ["export function filterNotes(notes, query) {\n", "  return notes.filter(note => note.title.includes(query));\n", "}\n"];
const after = ["export function filterNotes(notes, query) {\n", "  const keyword = query.trim().toLocaleLowerCase();\n", "  if (!keyword) return notes;\n", "  return notes.filter(note =>\n", "    note.title.toLocaleLowerCase().includes(keyword)\n", "  );\n", "}\n"];
const snapshot: ReviewSnapshot = { id: "readme-snapshot", taskId, roots: [{
  root: { id: "garden", path: "/projects/garden-notes", git: true, scopes: ["main"] },
  mode: "uncommitted", base: "HEAD", target: "working", error: null, untracked: [paths[2]],
  files: paths.map((path, index) => ({ path, kind: index === 2 ? "added" : "modified", before: index === 2 ? null : { hash: "before", bytes: 120, kind: "file", executable: false, binary: false }, after: { hash: "after", bytes: 240, kind: "file", executable: false, binary: false } })),
  statistics: Object.fromEntries(paths.map((path, index) => [path, { additions: [18, 24, 32, 4][index], deletions: [3, 5, 0, 1][index], binary: false, error: null }])),
}] };
const review: TurnReviewRun = {
  request: { taskId, turnId: null, sourceSnapshot: snapshot.id, kind: "code", executor: executors[2], runId: "run_01K00000000000000000000001", retryOf: null, recheckOf: null },
  status: "completed", workspaceReview: true, createdAt: instant, finishedAt: instant,
  nativeSessionId: null, sourceSequence: 6, sourceArtifact: null, inputSha256: "demo", coverage: "", error: null,
  result: "已检查搜索逻辑、键盘操作和测试。建议补充空白搜索与大小写处理。",
  report: { schemaVersion: 1, structured: true, files: [], warnings: [], findings: [{
    fileId: paths[0], path: paths[0], rootId: "garden", severity: "medium", title: "搜索应忽略首尾空格与大小写",
    description: "直接使用 includes(query) 会让带空格或不同大小写的关键词漏掉匹配笔记。",
    suggestion: "规范化关键词与标题，并补充空搜索、空格和大小写测试。", side: "after", startLine: 2, endLine: 4,
    evidence: "note.title.includes(query)", locationVerified: true, locationNote: "",
  }] },
};
const scenario = new URLSearchParams(location.search).get("scene") ?? "overview";
const readOnly = async (): Promise<never> => { throw new Error("文档演示只展示界面，不执行操作"); };
class DocumentationClient extends DemoCoreClient {
  override async bootstrap(): Promise<BootstrapSnapshot> {
    const base = await super.bootstrap();
    return { ...base, lastSequence: 6, projects: [
      { projectId, displayName: "Garden Notes", path: "/projects/garden-notes", additionalDirectories: [], taskCount: 3 },
      { projectId: "prj_01K00000000000000000000002", displayName: "Design System", path: "/projects/design-system", additionalDirectories: [], taskCount: 1 },
    ], tasks: ["添加笔记搜索和键盘导航", "检查无障碍与边界条件", "完善移动端布局", "整理组件色彩变量"].map((title, index) => ({
      taskId: `tsk_${String(index + 1).padStart(26, "0")}` as TaskId,
      projectId: index === 3 ? "prj_01K00000000000000000000002" as const : projectId, title, preferredExecutor: executors[index], lastTurnExecutor: executors[index], activePrimaryRun: null, streamVersion: 6, updatedAt: instant,
    })).map((task, index) => index === 0 ? { ...task, taskId } : task),
    installations: executors.map((executor, index) => ({ providerId: executor.providerId,
      executable: `/demo/agents/${executor.providerId}`, fingerprint: `demo-${index}`, status: "ready", version: "demo", versionText: "文档演示", diagnostic: null, probeElapsedMillis: 0, probedAt: instant, installationId: executor.installationId as `ins_${string}` })),
    companion: { ...base.companion, workspaces: [], revision: 6 }, activeRuns: [], sessionMaps: [] };
  }
  override async timeline(selected: TaskId): Promise<TimelineItem[]> {
    const user = (itemId: string, text: string, sequence: number): TimelineItem => ({ itemId, taskId: selected, turnId: `trn_${String(sequence).padStart(26, "0")}`, kind: "user_message", text, providerId: null, sequence, occurredAt: instant, attachments: [] });
    const response = (itemId: string, text: string, sequence: number, providerId: string): TimelineItem => ({ ...user(itemId, text, sequence), turnId: `trn_${String(sequence - 1).padStart(26, "0")}`, kind: "assistant_message", providerId,
      turnCompletion: { startedAt: instant, completedAt: "2026-10-17T09:31:12.000Z", firstOutputMs: 1200, usage: { scope: "native_turn", models: ["gpt-5.4"], requestCount: 3, inputTokens: 8420, outputTokens: 1240, cacheReadTokens: 6020, cacheWriteTokens: null, reasoningTokens: 420, costUsd: 0.028, credits: null, outputTokensPerSecond: 46.2, partial: false } } });
    const items = [user("question", "给笔记列表加上搜索：支持键盘快捷键、大小写不敏感，并补齐测试。", 1),
      response("implementation", "已完成搜索与键盘导航。\n\n- 支持 **⌘ K / Ctrl K** 聚焦、Escape 清空。\n- 搜索忽略大小写，保留原有标签和排序。\n- 空搜索、键盘操作与无结果状态测试通过。\n\n```tsx\nconst keyword = query.trim().toLocaleLowerCase();\nconst visible = notes.filter(note =>\n  note.title.toLocaleLowerCase().includes(keyword)\n);\n```", 2, "codex"),
      user("follow-up", "请让另一个 Agent 检查实现，重点看搜索边界和键盘可访问性。", 3),
      response("review-reply", "已核对实现与测试。搜索框标签、键盘聚焦及空状态处理完整；建议补充输入法组合期间的快捷键测试。\n\n修改范围：`SearchBox.tsx`、`search.css` 和搜索测试。", 4, "cursor")];
    return scenario === "switch-agent" ? items : items.slice(0, 2);
  }
  async availableModels(): Promise<ModelCatalog> { return catalog; }
  async modelReadyExecutors() { return executors; }
  async reviewExecutors() { return executors; }
  async turnReviews() { return scenario === "review-fix" ? [review] : []; }
  override async quotaSnapshots() {
    return (await super.quotaSnapshots()).map(view => ({ ...view, installationId: executors[0].installationId! }));
  }
  override submitTurn = readOnly;
  override cancelTurn = readOnly;
}
const client = new DocumentationClient();
const runCatalog: ProjectRunCatalog = { projectId, platform: "macos", shell: "zsh", generation: "docs-v1", executor: executors[0], directories: [{
  path: "/projects/garden-notes", status: "ready", error: null, commands: [
    { id: "dev", label: "启动开发服务", command: "pnpm dev", cwd: ".", source: "package.json", pinned: true },
    { id: "test", label: "运行测试", command: "pnpm test", cwd: ".", source: "package.json" },
    { id: "build", label: "构建生产版本", command: "pnpm build", cwd: ".", source: "package.json" },
  ],
}] };
const terminal: TerminalInfo = { id: "documentation-terminal", epoch: "docs", projectId, directoryIndex: 0, taskId, commandId: "dev", label: "启动开发服务", cwd: "/projects/garden-notes", command: "pnpm dev", status: "running", exitCode: null, error: null, outputEnd: 150, createdAt: 0 };
const connect = async (receive: (event: TerminalOutput) => void) => {
  receive({ kind: "state", info: terminal });
  receive({ kind: "data", bytes: [...new TextEncoder().encode("\r\n  VITE  ready in 184 ms\r\n\r\n  ➜  Local:   http://localhost:5173/\r\n  ➜  press h + enter to show help\r\n")] });
  return { id: terminal.id, detach: () => {} };
};
projectRunClient.restore = (): RunProcess => ({ id: terminal.id, stop: readOnly, api: { ...terminalClient, open: async (_project, _index, _cols, _rows, receive) => connect(receive), attach: async (_id, receive) => connect(receive), input: readOnly, close: readOnly, resize: async () => {} } });

// An allowlist of in-memory reads only. No requests reach a desktop service.
let callback = 0;
const invoke = async (command: string, args: Record<string, unknown> = {}) => {
  switch (command) {
    case "core_bootstrap": return client.bootstrap();
    case "core_available_models": return catalog;
    case "core_model_ready_executors": case "core_review_executors": return executors;
    case "core_turn_reviews": return client.turnReviews();
    case "core_review_workspaces": return [{ taskId: args.taskId, name: "Garden Notes", conversationTitle: "添加笔记搜索和键盘导航", path: "/projects/garden-notes", branch: "main", head: "a1b2c3d", changedFiles: 4, current: true, primary: true, git: true, running: false, error: null }];
    case "core_review_query": return snapshot;
    case "core_review_file": return { binary: false, text: { before, after, additions: 5, deletions: 1, blocks: [{ kind: "equal", oldStart: 0, oldEnd: 1, newStart: 0, newEnd: 1 }, { kind: "replace", oldStart: 1, oldEnd: 2, newStart: 1, newEnd: 6 }, { kind: "equal", oldStart: 2, oldEnd: 3, newStart: 6, newEnd: 7 }] } };
    case "core_review_snapshot_lease": return;
    case "core_review_history": case "core_review_branches": return [];
    case "core_project_runs": return runCatalog;
    case "core_terminal_list": return scenario === "run-project" ? [terminal] : [];
    case "core_review_watch": case "core_review_unwatch": case "core_project_runs_watch": case "core_project_runs_unwatch": case "core_subscribe": case "core_unsubscribe": return;
    default: throw new Error(`文档演示未提供此操作：${command}`);
  }
};
(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = { invoke, transformCallback: () => ++callback, unregisterCallback: () => {} };
localStorage.setItem("dogcodes.welcome.completed.v1", "true");
localStorage.setItem("dogcodes.conversation.lastViewed.v1", taskId);
localStorage.setItem("dogcodes.review.width", scenario === "git-changes" ? "840" : "540");
localStorage.setItem("dogcodes.sidebar.width", scenario === "projects" ? "350" : "250");
document.documentElement.dataset.theme = "light";
createRoot(document.getElementById("root")!).render(<><Tooltips /><App client={client} /><span style={{ position: "fixed", bottom: 7, right: 10, zIndex: 10000, fontSize: 10, color: "#777", pointerEvents: "none" }}>文档演示 · 隔离示例数据</span></>);
