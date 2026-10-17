import { useState } from "react";
import { createRoot } from "react-dom/client";
import type { CoreClient, ForkRequest, TimelineItem } from "@dogcodes/protocol";
import { ConversationContent } from "../ConversationActivity";
import { Tooltips } from "../Tooltips";
import "../styles.css";
import "../conversation.css";
const date = "2026-10-16T12:00:00Z";
const items: TimelineItem[] = [{ itemId: "final", taskId: "tsk_preview", turnId: "trn_preview", kind: "assistant_message", text: "已完成本轮修改。你可以从这里 Fork 对话，在当前工作区或新的 worktree 中继续。", providerId: "codex", attachments: [], sequence: 1, occurredAt: date, turnCompletion: { startedAt: date, completedAt: date } }];
function Fixture() {
  const [result, setResult] = useState("");
  const [theme, setTheme] = useState("light");
  document.documentElement.dataset.theme = theme;
  const client = { forkTask: async (request: ForkRequest) => { setResult(`已选择：${request.environment === "worktree" ? "新 Worktree" : "当前工作区"}`); return request.newTaskId; } } as unknown as CoreClient;
  return <main style={{ maxWidth: 800, margin: "80px auto", padding: 24 }}>
    <button type="button" onClick={() => setTheme(theme === "light" ? "dark" : "light")}>切换主题</button>
    <div style={{ marginTop: 160 }}><ConversationContent items={items} client={client} onSubagent={() => {}} renderEntry={item => <p>{item.text}</p>}/></div>
    <p role="status">{result}</p>
  </main>;
}
createRoot(document.getElementById("root")!).render(<><Fixture/><Tooltips/></>);
