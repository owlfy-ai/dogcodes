import { useState } from "react";
import { createRoot } from "react-dom/client";
import type { CoreClient, ForkRequest, ProjectId, TaskId, TaskSummary, TimelineItem } from "@dogcodes/protocol";
import { ConversationHistory } from "../ConversationHistory";
import { ConversationContent } from "../ConversationActivity";
import { Tooltips } from "../Tooltips";
import "../styles.css";
import "../conversation.css";
const date = "2026-10-16T12:00:00Z";
const items: TimelineItem[] = [{ itemId: "final", taskId: "tsk_preview", turnId: "trn_preview", kind: "assistant_message", text: "已完成本轮修改。你可以从这里 Fork 对话，在当前工作区或新的 worktree 中继续。", providerId: "codex", attachments: [], sequence: 1, occurredAt: date, turnCompletion: { startedAt: date, completedAt: date } }];
const forkItem: TimelineItem = { itemId: "fork", taskId: "tsk_FORK", turnId: null, kind: "fork_context", providerId: null, attachments: [], sequence: 1, occurredAt: date,
  text: JSON.stringify({ sourceTaskId: "tsk_SOURCE", environment: "worktree", restoredTurnSnapshot: true, text: JSON.stringify([
    { kind: "turn.queued", text: "弹出一个卡片让我选择一下今晚吃什么" }, { kind: "message.delta", text: "好的，这里有三个选项：**火锅**、寿司和意面。" },
  ]) }) };
const executor = { providerId: "codex", installationId: null };
const project = { projectId: "prj_DRILL" as ProjectId, displayName: "DrillHUD", path: "/projects/DrillHUD", additionalDirectories: [], taskCount: 3 };
const worktree = { ...project, projectId: "prj_FORK" as ProjectId, displayName: "DrillHUD · Fork", path: "/projects/DrillHUD/.git/dogcodes-worktrees/tsk_FORK" };
const task = (taskId: string, title: string, extra: Partial<TaskSummary> = {}): TaskSummary => ({ taskId: taskId as TaskId, projectId: project.projectId, title, preferredExecutor: executor, activePrimaryRun: null, streamVersion: 1, ...extra });
const tasks = [task("tsk_HELLO", "你好"), task("tsk_SOURCE", "弹出一个卡片让我选择一下今晚吃什么"),
  task("tsk_FORK", "弹出一个卡片让我选择一下今晚吃什么 · Fork", { projectId: worktree.projectId, forkedFrom: "tsk_SOURCE" as TaskId }),
  task("tsk_FORK2", "换成只看日料", { forkedFrom: "tsk_SOURCE" as TaskId }), task("tsk_LANG", "这个项目是什么语言开发的")];
function Fixture() {
  const [result, setResult] = useState("");
  const [theme, setTheme] = useState("light");
  document.documentElement.dataset.theme = theme;
  const client = { forkTask: async (request: ForkRequest) => { setResult(`已选择：${request.environment === "worktree" ? "新 Worktree" : "当前工作区"}`); return request.newTaskId; } } as unknown as CoreClient;
  return <main style={{ maxWidth: 1100, margin: "80px auto", padding: 24 }}>
    <button type="button" onClick={() => setTheme(theme === "light" ? "dark" : "light")}>切换主题</button>
    <div style={{ marginTop: 160 }}><ConversationContent items={items} client={client} onSubagent={() => {}} renderEntry={item => <p>{item.text}</p>}/></div>
    <p role="status">{result}</p>
    <div style={{ display: "grid", gridTemplateColumns: "260px 1fr", gap: 24, marginTop: 40 }}>
      <aside className="sidebar" style={{ padding: 8 }}><ConversationHistory projects={[project, worktree]} tasks={tasks} selectedTaskId={"tsk_FORK" as TaskId} onSelect={() => {}}/></aside>
      <div className="conversation-flow"><ConversationContent items={[forkItem]} client={client} onSubagent={() => {}} renderEntry={item => <p>{item.text}</p>} taskTitle={id => tasks.find(t => t.taskId === id)?.title} onOpenTask={id => setResult(`打开 ${id}`)}/></div>
    </div>
  </main>;
}
createRoot(document.getElementById("root")!).render(<><Fixture/><Tooltips/></>);
