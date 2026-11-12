import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { displayBackendText, localizeBackendText } from "./backendText";
import { backendMessages } from "./backendTextCatalog";
import { setGlobalResolvedLanguage, type ResolvedLanguage } from "./i18n";
import { LOCALES } from "./i18n/index";
import { userFacingError } from "./userFacingError";

// Everything the management of agents and remote hosts can say to a person.
const REMOTE_SOURCES = [
  "crates/workspace/src/review/recovery/turn.rs", "crates/workspace-ops/src/turn_undo.rs",
  "apps/desktop/src-tauri/src/clipboard.rs", "apps/desktop/src-tauri/src/execution_targets.rs", "apps/desktop/src-tauri/src/execution_targets/agent_installs.rs",
  "apps/desktop/src-tauri/src/execution_targets/agent_updates.rs", "apps/desktop/src-tauri/src/execution_targets/agents.rs",
  "apps/desktop/src-tauri/src/terminal_host/remote.rs", "apps/desktop/src-tauri/src/daemon/remote.rs", "crates/antigravity-driver/src/approval_bridge.rs",
  "crates/agent-management/src/install.rs", "crates/agent-management/src/operation.rs", "crates/agent-management/src/http.rs",
  "crates/agent-management/src/updates.rs", "crates/agent-management/src/updates/codex_host.rs", "crates/agent-management/src/updates/electron_host.rs",
  "crates/agent-management/src/updates/kiro.rs", "crates/agent-management/src/updates/managed_cli.rs", "crates/agent-management/src/updates/new_agents.rs",
];
// Update sources are translated word by word by `localizedUpdateSource`, approval options by
// their role, and the last four by `localizedAgentMessage` as they stand.
const TRANSLATED_ELSEWHERE = new Set([
  "Kiro CLI 原生更新器", "Kiro 原生更新器", "Pi 原生更新器", "Grok 原生更新器", "Claude 原生更新器", "Kimi 原生更新器", "OpenCode 原生更新器",
  "外部安装", "桌面应用", "ZCode 桌面应用", "npm（当前 npx 安装）", "Codex 桌面应用 · 官方更新源", "允许一次", "拒绝",
  "更新命令未成功（退出码 {}），请检查网络和安装权限后重试", "未知", "桌面应用可升级至 {latest}，请在 ZCode 中完成升级", "桌面应用已是最新版本",
  "电脑", "远程连接失败：{e:?}",
]);
const anyParameter = (text: string) => text.replace(/\{\w*\}/g, "{}");

describe("backend text localization", () => {
  it("has translations for agent management, remote hosts and whole-turn undo messages", () => {
    const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
    const known = new Set(backendMessages.map(([zh]) => anyParameter(zh)));
    const missing = REMOTE_SOURCES.flatMap(source => readFileSync(join(root, source), "utf8").split("#[cfg(test)]")[0].split("\n")
      .filter(line => !line.trim().startsWith("//"))
      .flatMap(line => [...line.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map(match => match[1]))
      .filter(text => /\p{Script=Han}/u.test(text) && !TRANSLATED_ELSEWHERE.has(text) && !known.has(anyParameter(text)))
      .map(text => `${source}: ${text}`));
    expect(missing).toEqual([]);
  });

  it("gives parameters back in every language", () => {
    for (const lang of Object.keys(LOCALES) as ResolvedLanguage[]) {
      expect(localizeBackendText("DeepSeek Harness 需要 Node.js 24.9 或更高版本，这台主机当前是 v22.3.0。请升级后重试", lang)).toContain("v22.3.0");
      expect(localizeBackendText("安装程序未成功（退出码 35），请检查这台主机的网络和权限后重试", lang)).toContain("35");
    }
  });

  it("translates every catalog message and keeps zh-CN verbatim", () => {
    for (const [zh] of backendMessages.filter(([text]) => !text.includes("{"))) {
      expect(localizeBackendText(zh, "en")).not.toMatch(/\p{Script=Han}/u);
      expect(displayBackendText(zh, "zh-CN")).toBe(zh);
    }
  });

  it("names the agent in the one-task-at-a-time queue notice", () => {
    const zh = "Antigravity同一时间只能执行一个任务，之前的任务执行完成后会开始执行此任务";
    expect(localizeBackendText(zh, "en")).toBe("Antigravity can only run one task at a time. This task will start automatically when the previous one finishes.");
    expect(displayBackendText(zh, "zh-CN")).toBe(zh);
  });

  it("translates remote pairing, terminal and analysis failures", () => {
    expect(localizeBackendText("手机配对连接未完成，请核实或重新生成邀请。", "en")).toBe("Phone pairing didn't finish. Check it or generate a new invitation.");
    expect(localizeBackendText("接入服务暂时无法处理请求（HTTP 503），请稍后重试", "en")).toContain("503");
    expect(localizeBackendText("远程连接失败：timeout", "ja")).toContain("timeout");
    expect(localizeBackendText("无法核实退出结果：reaper unavailable", "de")).toContain("reaper unavailable");
    expect(displayBackendText("项目分析已中断，请重新分析；未自动重新执行", "zh-CN")).toBe("项目分析已中断，请重新分析；未自动重新执行");
  });

  it("translates parameterized and agent-error-wrapped messages", () => {
    expect(localizeBackendText("Coding Agent 额度已用尽；预计于 2026-01-02 03:04 UTC 重置，也可以切换 Agent。", "en")).toContain("2026-01-02 03:04 UTC");
    const wrapped = localizeBackendText("Agent 请求暂时受限，请稍后重试。\n\nAgent 返回的错误：\n429 too many", "en");
    expect(wrapped).toContain("rate limited");
    expect(wrapped).toContain("429 too many");
  });

  it("translates undo errors, nested failures and recovery guidance in every language", () => {
    for (const lang of Object.keys(LOCALES) as ResolvedLanguage[]) {
      for (const message of ["本轮快照内容不完整，无法撤销全部修改", "此记录不是完整的一轮修改，无法整轮撤销"]) {
        expect(localizeBackendText(message, lang)).toBeTruthy();
        if (lang !== "zh-CN") expect(localizeBackendText(message, lang)).not.toBe(message);
      }
      const conflict = "src/example.ts 在本轮结束后又有修改，未撤销任何文件";
      const translated = localizeBackendText(conflict, lang)!;
      expect(translated).toContain("src/example.ts");
      for (const recovery of ["已恢复已处理的文件；备份保存在恢复记录中", "备份已保留，请在审查的恢复记录中处理"]) {
        const wrapped = localizeBackendText(`撤销未完成：${conflict}。${recovery}`, lang)!;
        expect(wrapped).toContain(translated);
        expect(wrapped).toContain(localizeBackendText(recovery, lang));
        expect(wrapped).not.toMatch(/\{\w+\}/);
      }
    }
  });

  it("never leaks unmapped Chinese through userFacingError outside zh-CN", () => {
    setGlobalResolvedLanguage("en");
    try {
      expect(userFacingError("某个没有映射的后端错误", "fallback")).toBe("fallback");
      expect(userFacingError("附件已不可读取", "fallback")).toBe("The attachment can no longer be read.");
    } finally {
      setGlobalResolvedLanguage("zh-CN");
    }
    expect(userFacingError("某个没有映射的后端错误", "fallback")).toBe("某个没有映射的后端错误");
  });
});
