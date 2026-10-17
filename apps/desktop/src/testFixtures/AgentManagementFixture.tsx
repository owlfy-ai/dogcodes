import { AgentPickerMenu } from "../AgentPickerMenu";
// Isolated UI fixture. Actions only change local fixture state; no native commands.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import type { AgentUpdate, InstallationSummary, QuotaSnapshotView, QuotaWindow } from "@dogcodes/protocol";
import { QuotaCenter } from "../App";
import { Tooltips } from "../Tooltips";
import type { QuotaRefreshProgress } from "../quotaRefresh";
import "../styles.css";

const now = Date.now();
const windowFor = (id: string, usedPercent: number | null, windowMinutes: number, scope?: string): QuotaWindow => ({ id, usedPercent, windowMinutes, scope, resetsAtUnix: Math.floor(now / 1000) + 18000 });
const data = [
  { providerId: "antigravity", version: "1.2.14", plan: "订阅额度", windows: [windowFor("gemini5", .1, 300, "Gemini Models"), windowFor("claude5", 0, 300, "Claude and GPT models"), windowFor("gemini7", 1.2, 10080, "Gemini Models"), windowFor("claude7", 0, 10080, "Claude and GPT models")] },
  { providerId: "codex", version: "0.158.0-alpha.2.1", plan: "Pro", windows: [windowFor("weekly", 7, 10080)] },
  { providerId: "cursor", version: "2026.9.23-86fc751", plan: "Pro · 按量付费关闭", windows: [windowFor("cursor-models", 20, 0, "Cursor Models"), windowFor("cursor-other-models", 100, 0, "Other Models")] },
  { providerId: "grok", version: "1.0.46", plan: "SuperGrok", windows: [windowFor("weekly", 2, 10080)] },
  { providerId: "kimi", version: "2.1.1", plan: "订阅额度", windows: [windowFor("5h", 0, 300), windowFor("weekly", 4.1, 10080)] },
  { providerId: "claude-code", version: "2.1.266", plan: null, windows: null },
];
const installations: InstallationSummary[] = data.map(agent => ({ installationId: `ins_${agent.providerId}`, providerId: agent.providerId, executable: "fixture", fingerprint: "fixture", status: "ready", version: agent.version, versionText: agent.version, diagnostic: null, probeElapsedMillis: 0, probedAt: new Date(now).toISOString() }));
const views: QuotaSnapshotView[] = data.map(agent => ({ installationId: `ins_${agent.providerId}`, providerId: agent.providerId, quota: agent.windows ? { windows: agent.windows, context: null, planType: agent.plan, source: "native_pull", confidence: "provider_reported", collectedAtUnixMs: now, staleAfterUnixMs: now + 3600000, parserVersion: "fixture" } : null, diagnostic: agent.windows ? null : "claude_quota_login_required" }));
const update = (id: string, status: AgentUpdate["status"], latestVersion: string, desktopVersion?: string): AgentUpdate => ({ installationId: `ins_${id}`, status, currentVersion: data.find(agent => agent.providerId === id)!.version, latestVersion, desktopVersion, message: null, source: "fixture", canUpgrade: status === "available", checkedAt: now });
document.documentElement.dataset.theme = "light";
function Fixture() {
  const [progress, setProgress] = useState<QuotaRefreshProgress | null>(null);
  const [notice, setNotice] = useState("");
  const [menu, setMenu] = useState(false);
  const [empty, setEmpty] = useState(false);
  const [narrow, setNarrow] = useState(false);
  const [updates, setUpdates] = useState({ ins_codex: update("codex", "managed", "26.928.31416", "26.924.22138"), ins_cursor: update("cursor", "available", "2026.09.28-64d2043") });
  return <><div style={{ display: "flex", gap: 10, padding: 10, fontSize: 12, flexWrap: "wrap" }}>
    <button onClick={() => { document.documentElement.dataset.theme = document.documentElement.dataset.theme === "light" ? "dark" : "light"; }}>切换主题</button>
    <button onClick={() => setEmpty(value => !value)}>切换空状态</button>
    <button onClick={() => setNarrow(value => !value)}>切换窄屏</button>
    <div style={{position:"relative"}}><button onClick={() => setMenu(value => !value)}>预览 Agent 下拉列表</button>
      {menu && <AgentPickerMenu options={(empty ? [] : installations).filter(item => item.status === "ready").map(executor => ({ key: executor.providerId, executor }))} selectedKey="" title="选择 Agent" label="管理页菜单预览" placement="bottom" onSelect={option => { setNotice(`已选择 ${option.executor.providerId}（模拟）`); setMenu(false); }} />}</div>
    {notice && <span role="status">{notice}</span>}
  </div><main style={{ height: "calc(100vh - 48px)", width: narrow ? "min(520px, 100%)" : "100%", margin: "auto", display: "grid" }}>
    <QuotaCenter views={empty ? [] : views} installations={empty ? [] : installations} agentUpdates={{ updates, check: () => setNotice("已检查版本（模拟）"), upgrade: () => { setUpdates(old => ({ ...old, ins_cursor: { ...old.ins_cursor, status: "updated" } })); setNotice("升级完成（模拟）"); }, openUpdater: () => setNotice("已打开宿主应用（模拟）") }} refreshProgress={progress} onRefresh={() => { setProgress({ running: true, completed: 0, total: 6, unavailable: 0, failed: false }); setTimeout(() => setProgress({ running: false, completed: 6, total: 6, unavailable: 1, failed: false }), 800); }} />
  </main><Tooltips /></>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
