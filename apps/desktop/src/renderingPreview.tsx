import { ContextUsageRing } from "./ContextUsageRing";
import { Tooltips } from "./Tooltips";
/// <reference types="vite/client" />
// Development-only visual acceptance entry, not imported by the application.
import { useState } from "react";
import { PermissionPicker } from "./PermissionPicker";
import { createRoot } from "react-dom/client";
import type { TimelineItem } from "@dogcodes/protocol";
import { ConversationContent, SubagentPanel } from "./ConversationActivity";
import { MarkdownMessage } from "./MarkdownMessage";
import { DemoCoreClient } from "./demoClient";
import "./styles.css";
const client = Object.assign(new DemoCoreClient(), {permissionCatalog: async()=>({defaultModeId:"default",modes:[
{id:"plan",label:"计划模式",description:"探索并制定计划；使用 Agent 原生计划模式。",dangerous:false},
{id:"default",label:"默认",description:"编辑文件和其他受保护操作前请求审批。",dangerous:false},
{id:"acceptEdits",label:"接受编辑",description:"允许文件编辑；其他受保护操作仍需审批。",dangerous:false},
{id:"auto",label:"自动判断",description:"由 Agent 自动分类权限请求。",dangerous:false},
{id:"bypassPermissions",label:"跳过权限检查",description:"跳过工具权限检查。",dangerous:true},
]})});
const row = (id: string, data: object): TimelineItem => ({attachments: [], 
  itemId: `tool:tsk_preview:trn_preview:${id}`,
  taskId: "tsk_preview",
  turnId: "trn_preview",
  kind: "activity",
  text: JSON.stringify(data),
  providerId: "claude-code",
  sequence: 1,
  occurredAt: new Date().toISOString(),
});
const items = [
  row("thought", { type: "thought", status: "completed", durationMs: 6800 }),
  row("intro", {
    type: "message",
    text: "## 优化 Agent 的对话体验\n\n我先检查了 **事件映射** 与 `TimelineEntry`，并委派一个子智能体核对协议。",
  }),
  row("read1", {
    type: "tool",
    name: "Read",
    status: "completed",
    input: { file_path: "src/App.tsx" },
    output: "export function App() { … }",
  }),
  row("search", {
    type: "tool",
    name: "Grep",
    status: "completed",
    input: { pattern: "ToolCallUpdate" },
    output: "3 matches",
  }),
  row("child", {
    type: "subagent",
    title: "检查子智能体事件映射",
    status: "completed",
    details: "available",
  }),
  row("childread", {
    type: "tool",
    name: "Read",
    parentId: "child",
    status: "completed",
    input: { file_path: "src/protocol.ts" },
    output: "父调用关联字段已验证。",
  }),
  row("childmessage", {
    type: "message",
    parentId: "child",
    text: "### 核对结果\n\n- [x] 父子 ID 关联正确\n- [x] 工具结果可以归属\n\n使用 `parent_tool_use_id` 建立关联，后续事件更新同一条记录。",
  }),
  row("edit", {
    type: "tool",
    name: "Edit",
    status: "completed",
    input: {
      file_path: "src/App.tsx",
      old_string: "return <p>{text}</p>;\n",
      new_string: "return <MarkdownMessage text={text} />;\n",
    },
  }),
  row("terminal", {
    type: "tool",
    name: "Bash",
    status: "completed",
    input: { command: "pnpm test" },
    output: "7 tests passed",
  }),
  row("final", {
    type: "message",
    text: '### 渲染结果\n\n| 内容 | 展示 |\n| --- | --- |\n| Markdown | 标题、列表、表格 |\n| Mermaid | 图表与源码 |\n| LaTeX | 行内与独立公式 |\n\n行内公式 $E=mc^2$，以及：\n\n$$\n\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\n$$\n\n```mermaid\nflowchart LR\n A[原生事件] --> B[统一投影]\n B --> C[主对话]\n B --> D[子智能体侧栏]\n```\n\n```typescript\nconst completed = tools.filter(tool => tool.status === "completed");\n```',
  }),
  row("unavailable", {
    type: "subagent",
    title: "另一个 Agent 的委派任务",
    status: "completed",
    details: "unavailable",
  }),
];
function Preview() {
  const [selected, setSelected] = useState<string | null>(null);
  return (
    <main
      style={{
        display: "flex",
        height: "100vh",
        maxWidth: 1500,
        margin: "auto",
        overflow: "hidden",
      }}
    >
      <section
        style={{ flex: 1, minWidth: 0, overflow: "auto", padding: "32px 48px" }}
      >
        <button
          onClick={() => {
            document.documentElement.dataset.theme =
              document.documentElement.dataset.theme === "light"
                ? "dark"
                : "light";
          }}
        >
          切换主题
        </button>
        <ConversationContent
          items={items}
          client={client}
          onSubagent={setSelected}
          renderEntry={(i) => <MarkdownMessage key={i.itemId} text={i.text} />}
        />
        <div style={{position:"fixed",bottom:24,left:80,zIndex:100,padding:12,borderRadius:16,background:"var(--panel-raised)"}}>
          <ContextUsageRing providerId="codex" quota={{ windows: [], collectedAtUnixMs: Date.now(), staleAfterUnixMs: Date.now() + 60000, source: "preview", confidence: "provider_reported", parserVersion: "preview", sessionUsage: { contextUsedTokens: 157624, contextWindowTokens: 258400, cacheHitRatePercent: 99.2, cacheReadTokens: 640500000, cacheWriteTokens: 0, reasoningTokens: 708500, totalTokens: 657000000, inputTokens: 654400000, outputTokens: 2600000, totalsScope: "session", observedAtUnixMs: Date.now() } }} />
          <PermissionPicker client={client} executor={{providerId:"claude-code",installationId:"ins_preview"}} taskId="tsk_preview" disabled={false} onChange={()=>{}} />
        </div>
      </section>
      {selected && (
        <SubagentPanel
          key={selected}
          item={items.find((i) => i.itemId === selected)!}
          items={items}
          onClose={() => setSelected(null)}
        />
      )}
    </main>
  );
}
document.documentElement.dataset.theme = "light";
const root = createRoot(document.getElementById("root")!);
root.render(<><Tooltips /><Preview /></>);
if (import.meta.hot) import.meta.hot.dispose(() => root.unmount());
