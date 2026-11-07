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
  "apps/desktop/src-tauri/src/clipboard.rs", "apps/desktop/src-tauri/src/execution_targets.rs", "apps/desktop/src-tauri/src/execution_targets/agent_installs.rs",
  "apps/desktop/src-tauri/src/execution_targets/agent_updates.rs", "apps/desktop/src-tauri/src/execution_targets/agents.rs",
  "apps/desktop/src-tauri/src/terminal_host/remote.rs", "crates/antigravity-driver/src/approval_bridge.rs",
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
]);
const anyParameter = (text: string) => text.replace(/\{\w*\}/g, "{}");

describe("backend text localization", () => {
  it("has a translation for all the text of agent and remote host management", () => {
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

  it("translates parameterized and agent-error-wrapped messages", () => {
    expect(localizeBackendText("Coding Agent 额度已用尽；预计于 2026-01-02 03:04 UTC 重置，也可以切换 Agent。", "en")).toContain("2026-01-02 03:04 UTC");
    const wrapped = localizeBackendText("Agent 请求暂时受限，请稍后重试。\n\nAgent 返回的错误：\n429 too many", "en");
    expect(wrapped).toContain("rate limited");
    expect(wrapped).toContain("429 too many");
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
