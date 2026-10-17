import { Tooltips } from "../Tooltips";
// Manual WebKit acceptance: real popovers above/below fractional-position anchors.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { AgentPickerMenu } from "../AgentPickerMenu";
import "../styles.css";
import "../review.css";
import "../reviewActions.css";
import "../projectRun.css";

document.documentElement.dataset.theme = "light";
const providers = ["antigravity", "claude-code", "codebuddy", "codex", "cursor", "deepseek-harness", "grok", "kimi", "kiro", "opencode", "pi", "zcode"];
const options = providers.map(providerId => ({ key: providerId, executor: { providerId, installationId: null } }));
function Fixture() {
  const [theme, setTheme] = useState("light");
  const [scale, setScale] = useState(1);
  const menu = (title: string, placement?: "bottom") => <AgentPickerMenu options={options} selectedKey="codex" title={title} label={title} onSelect={() => {}} placement={placement}/>;
  return <>
    <div style={{ position: "fixed", top: 8, left: 8 }}>
      <button onClick={() => { const next = theme === "light" ? "dark" : "light"; document.documentElement.dataset.theme = next; setTheme(next); }}>切换主题：{theme}</button>
      <button onClick={() => { const next = scale === 1 ? 1.25 : 1; document.documentElement.style.setProperty("--ui-scale", String(next)); setScale(next); }}>界面缩放：{scale}</button>
    </div>
    <div className="composer" style={{ position: "fixed", left: 282.25, top: 695.25, width: 40, padding: 0 }}>
      <details className="agent-picker" open><summary>主对话</summary>{menu("选择下一轮 Agent")}</details>
    </div>
    <aside className="review-panel" style={{ position: "fixed", left: 620.75, top: 60.75, width: 40, bottom: "auto", padding: 0, border: 0 }}>
      <div className="turn-review-agent"><button aria-expanded="true">审查</button>{menu("审查 Agent · 不切换主 Agent", "bottom")}</div>
    </aside>
    <aside className="review-panel" style={{ position: "fixed", left: 960.25, top: 60.25, width: 40, bottom: "auto", padding: 0, border: 0 }}>
      <div className="review-body"><nav style={{ width: 40, minWidth: 0 }}><section className="review-commit"><div className="review-commit-input"><button aria-expanded="true">提交</button>{menu("选择 Agent 生成", "bottom")}</div></section></nav></div>
    </aside>
    <aside className="review-panel" style={{ position: "fixed", left: 1280.5, top: 60.5, width: 40, bottom: "auto", padding: 0, border: 0 }}>
      <div className="project-run-agent"><button aria-expanded="true">运行</button>{menu("选择运行命令分析 Agent", "bottom")}</div>
    </aside>
  </>;
}
createRoot(document.getElementById("root")!).render(<><Tooltips /><Fixture/></>);
