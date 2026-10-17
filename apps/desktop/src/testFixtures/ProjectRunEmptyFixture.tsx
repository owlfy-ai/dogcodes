// Isolated visual fixture: never invokes native commands or launches a process.
import { useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { ProjectRunPanel } from "../ProjectRunPanel";
import { ReviewEmptyState } from "../ReviewEmptyState";
import { WorkspaceTabs } from "../WorkspaceTabs";
import { Tooltips } from "../Tooltips";
import type { ProjectRunApi, ProjectRunCatalog } from "../projectRunClient";
import "../styles.css";
import "../conversation.css";
import "../review.css";

document.documentElement.dataset.theme = "light";
const cases = { empty: "没有运行命令", cleared: "已清空运行项", mixed: "部分目录为空", analyzing: "分析中", error: "分析失败", noAgent: "没有可用 Agent", noDirectory: "没有项目目录" };
type Scenario = keyof typeof cases;
function Fixture() {
  const [scenario, setScenario] = useState<Scenario>("empty");
  const [view, setView] = useState<"changes" | "run">("run");
  const [analyses, setAnalyses] = useState(0);
  const api = useMemo<ProjectRunApi>(() => {
    const executor = { providerId: "antigravity", installationId: "fixture" };
    const catalog: ProjectRunCatalog = { projectId: "fixture", platform: "macos", shell: "sh", generation: "1", executor, directories: scenario === "noDirectory" ? [] : [
      { path: "/projects/scratch", status: scenario === "analyzing" ? "analyzing" : scenario === "error" ? "error" : "ready", commands: [], error: scenario === "error" ? "分析失败，请稍后重新分析。" : null, userModified: scenario === "cleared" },
      ...(scenario === "mixed" ? [{ path: "/projects/web", status: "ready" as const, error: null, commands: [{ id: "dev", label: "启动开发服务", command: "npm run dev", cwd: ".", source: "package.json" }] }] : []),
    ] };
    return { watch: () => () => {}, list: async () => [], restore: () => { throw new Error("验收页面没有进程"); }, hasPending: () => false, discardPending: () => {}, retryPending: async () => catalog, mutate: async () => catalog, get: async () => catalog, executors: async () => scenario === "noAgent" ? [] : [executor],
      analyze: async () => { setAnalyses(value => value + 1); return catalog; },
      edit: async () => catalog, launch: () => { throw new Error("验收页面不启动进程"); },
    };
  }, [scenario]);
  return <><div style={{ display: "flex", flexWrap: "wrap", gap: 12, padding: 10, fontSize: 12 }}>
    <select aria-label="验收场景" value={scenario} onChange={event => setScenario(event.target.value as Scenario)}>{Object.entries(cases).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select>
    <button onClick={() => { document.documentElement.dataset.theme = document.documentElement.dataset.theme === "light" ? "dark" : "light"; }}>切换主题</button>
    <span>分析次数：{analyses}</span>
  </div><div style={{ width: "min(460px, 100%)", height: "calc(100vh - 54px)", margin: "0 auto" }}>
    <aside className="review-panel review-standalone" style={{ position: "relative", top: "auto", right: "auto", bottom: "auto", width: "100%", height: "100%", margin: 0 }}>
      <header className="review-titlebar" data-tauri-drag-region="deep"><WorkspaceTabs value={view} onChange={next => { if (next === "changes" || next === "run") setView(next); }} /></header>
      {view === "run" ? <ProjectRunPanel key={scenario} taskId="fixture" visible api={api} /> : <ReviewEmptyState />}
    </aside>
  </div><Tooltips /></>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
