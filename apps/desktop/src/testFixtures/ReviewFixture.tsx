import { Tooltips } from "../Tooltips";
// Manual browser acceptance only. This entry is not referenced by the production app.
import { useState, type CSSProperties } from "react";
import { createRoot } from "react-dom/client";
import { ReviewPanel } from "../ReviewPanel";
import { ReviewResizeHandle } from "../ReviewResizeHandle";
import type { ReviewWorkspaceApi } from "../ReviewWorkspaceBrowser";
import type { ReviewSnapshot, reviewClient } from "../reviewClient";
import type { CoreClient, ReviewRepairCall, TurnReviewRun } from "@dogcodes/protocol";
import "../styles.css";
import "../conversation.css";
import "../review.css";
document.documentElement.dataset.theme = "light";
const workspaces = new URLSearchParams(window.location.search).has("workspaces");
const workspaceApi: ReviewWorkspaceApi = { list: async () => [
  { taskId: "visual", name: "Dogcodes", conversationTitle: "完善变更面板", path: "/Users/developer/Projects/Dogcodes", branch: "main", head: "abcdef123456", changedFiles: 3, current: true, primary: true, git: true, running: false, error: null },
  { taskId: "fork", name: "Dogcodes · Fork", conversationTitle: "从第 6 轮尝试另一种布局", path: "/Users/developer/Projects/Dogcodes/.git/dogcodes-worktrees/tsk_fork", branch: "codex/fork-tsk_01DEMO", head: "abcdef123456", changedFiles: 1, current: false, primary: false, git: true, running: false, error: null },
] };

const path = "packages/very-long-project-name/src/features/review/ConversationReview.tsx";
const snapshot: ReviewSnapshot = { id: "visual", taskId: "visual", roots: [{ root: { id: "repo", path: "/acceptance/project", git: true, scopes: [""] }, mode: "uncommitted", base: "HEAD", target: "working", error: null, statistics: { [path]: { additions: 1, deletions: 1, binary: false, error: null } }, untracked: [], files: [{ path, kind: "modified", before: { hash: "old", bytes: 26, kind: "file", executable: false, binary: false }, after: { hash: "new", bytes: 28, kind: "file", executable: false, binary: false } }] }] };
if (new URLSearchParams(window.location.search).has("rename")) {
  snapshot.roots[0].files[0].kind = "renamed";
  snapshot.roots[0].files[0].previousPath = "legacy/OriginalReview.tsx";
  snapshot.roots[0].untracked = [path];
}
if (new URLSearchParams(window.location.search).has("multi")) {
  snapshot.roots = [
    { name: "kids-vibe-coding", paths: ["src/components/LessonCard.tsx", "src/components/ProgressRing.tsx", "src/hooks/useLesson.ts", "README.md"] },
    { name: "highcode-parent", paths: ["psp-backend/src/main/java/service/UserService.java", "psp-backend/src/main/java/controller/AuthController.java"] },
    { name: "psp-front-new", paths: ["src/components/addStudent/index.vue", "src/styles/theme.css", "src/utils/legacy.ts"] },
  ].map((group, index) => ({ ...snapshot.roots[0], root: { ...snapshot.roots[0].root, id: `repo-${index}`, path: `/acceptance/${group.name}` }, untracked: index === 0 ? [group.paths[2]] : [], files: group.paths.map((path, i) => ({ ...snapshot.roots[0].files[0], path, kind: index === 2 && i === 2 ? "deleted" : i === 2 ? "added" : "modified" })), statistics: Object.fromEntries(group.paths.map((p, i) => [p, { additions: i === 2 && index === 2 ? 0 : 12 + i * 7, deletions: i === 2 && index === 0 ? 0 : 3 + i * 2, binary: false, error: null }])) }));
}
if (new URLSearchParams(window.location.search).has("untracked")) {
  snapshot.roots = snapshot.roots.map(root => ({ ...root, untracked: root.files.map(file => file.path), files: root.files.map(file => ({ ...file, kind: "added", before: null })) }));
}
if (new URLSearchParams(window.location.search).has("empty")) {
  snapshot.roots = snapshot.roots.map(root => ({ ...root, files: [], statistics: {}, untracked: [] }));
}
const api: typeof reviewClient = {
  batchFiles: async () => { throw new Error("验收页面不修改文件"); },
  lease: async () => {},
  query: async ({ taskId }) => !workspaces ? snapshot : { ...snapshot, taskId, id: `${taskId}-snapshot`, roots: snapshot.roots.map(root => ({ ...root, root: { ...root.root, path: taskId === "fork" ? "/Users/developer/Projects/Dogcodes/.git/dogcodes-worktrees/tsk_fork" : "/Users/developer/Projects/Dogcodes" } })) },
  file: async () => ({ binary: false, text: { before: ['const message = "before";\n'], after: ['const message = "after";\n'], additions: 1, deletions: 1, blocks: [{ kind: "replace", oldStart: 0, oldEnd: 1, newStart: 0, newEnd: 1 }] } }),
  history: async () => [], branches: async () => [], preview: async (_task, _snapshot, _root, _path, version) => ({ text: version === "head" ? "const original = true;" : "const current = true;", image: null }),
  reveal: async () => {}, ignore: async () => {},
  index: async () => { throw new Error("验收页面不修改索引"); },
  discard: async () => { throw new Error("验收用冲突：文件已被其他程序修改。没有修改磁盘。"); },
};
const reviewMode = new URLSearchParams(window.location.search).get("review");
const executor = { providerId: reviewMode === "network" ? "cursor" : "antigravity", installationId: "visual" };
const reviewRun = (runId: string, kind: "code" | "performance", createdAt: string, findings: number): TurnReviewRun => ({
  workspaceReview: true, status: "completed", createdAt, finishedAt: createdAt, nativeSessionId: null, sourceSequence: 1, sourceArtifact: null, inputSha256: runId, coverage: "", error: null,
  request: { taskId: "visual", turnId: null, sourceSnapshot: "visual", kind, executor, runId, retryOf: null },
  result: "审查已完成。",
  report: { schemaVersion: 1, structured: true, files: [], warnings: [], findings: Array.from({ length: findings }, (_, index) => ({
    fileId: path, path, rootId: "repo", severity: index ? "low" : "medium", title: index ? "未使用的变量" : "提交前缺少必填校验", description: "表单提交时没有检查学生姓名是否为空，后端会收到空字符串。",
    suggestion: "在 submit 前校验必填字段。", side: "after", startLine: 12 + index, endLine: 14 + index, evidence: index ? "" : "if (form.name) submit(form);", locationVerified: true, locationNote: "",
    verification: index ? null : { verdict: "confirmed", reason: "submit 调用前没有任何必填校验，空表单可直接提交。" },
  })) },
} as unknown as TurnReviewRun);
const reviewRuns = reviewMode === null ? [] : [reviewRun("latest", "performance", "2026-09-29T07:40:00Z", 0), reviewRun("older", "code", reviewMode === "findings" ? "2026-09-29T07:50:00Z" : "2026-09-29T07:20:00Z", 3)];
if (reviewMode === "network") {
  const failed = reviewRun("network-failure", "code", "2026-10-14T08:21:00Z", 0);
  failed.status = "failed";
  failed.error = "Error: RetriableError: [internal] HTTP/2 keepalive ping timed out after 5000ms";
  failed.result = `先核对未提交差异和相关调用方，接着检查脚本、配置和失败路径。\n\n${failed.error}`;
  failed.report = undefined;
  failed.finishedAt = "2026-10-14T08:24:01Z";
  reviewRuns.splice(0, reviewRuns.length, failed);
}
const analysisClient = {
  bootstrap: async () => ({ installations: [{ ...executor, status: "ready", version: "1.2.13" }], tasks: [{ taskId: "visual", preferredExecutor: { providerId: "codex" }, activePrimaryRun: null }], lastSequence: 0 }),
  reviewRepair: async (call: ReviewRepairCall) => { if (call.action === "get") return null; throw new Error("验收页面不提交修复"); },
  reviewExecutors: async () => [executor], turnReviews: async () => reviewRuns,
  startProjectReview: async () => { throw new Error("验收页面不执行审查"); },
} as unknown as CoreClient;
function Fixture() {
  const [clicks, setClicks] = useState(0);
  const [opened, setOpened] = useState(true), [width, setWidth] = useState(380);
  const docked = new URLSearchParams(window.location.search).has("docked");
  const controls = <><button id="background-action" onClick={() => setClicks(n => n + 1)}>背后操作次数：{clicks}</button><button onClick={() => { document.documentElement.dataset.theme = document.documentElement.dataset.theme === "light" ? "dark" : "light"; }}>切换主题</button></>;
  const panel = <ReviewPanel taskId="visual" running={false} visible={!docked || opened} animateEntrance={docked && opened} onClose={() => setOpened(false)} api={api} analysisClient={analysisClient} workspaceNavigation={workspaces} workspaceApi={workspaceApi} onOpenConversation={taskId => window.alert(`打开会话：${taskId}`)} />;
  if (docked) return <main className={`app-shell${opened ? " review-expanded" : ""}`} style={{ "--sidebar-width": "220px", "--review-width": `${width}px` } as CSSProperties}>
    <aside className="sidebar"><strong>Dogcodes</strong><p>完善变更面板</p></aside>
    <section className="global-content" style={{ padding: 28 }}><h2>会话</h2><p>工作区切换位于右侧边栏中。</p>{controls}<button onClick={() => setOpened(value => !value)}>切换右侧边栏</button></section>
    {opened && <ReviewResizeHandle width={width} max={900} onResize={setWidth} />}{panel}
  </main>;
  return <>{controls}{panel}</>;
}
createRoot(document.getElementById("root")!).render(<><Tooltips /><Fixture /></>);
