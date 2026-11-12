import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { displayBackendText, localizeBackendText } from "./backendText";
import { setGlobalResolvedLanguage, type ResolvedLanguage } from "./i18n";
import { LOCALES } from "./i18n/index";
import { userFacingError } from "./userFacingError";

// Everything the management of agents and remote hosts can say to a person.
const REMOTE_SOURCES = [
  "crates/workspace/src/review/recovery/turn.rs", "crates/workspace-ops/src/turn_undo.rs",
  "apps/desktop/src-tauri/src/clipboard.rs", "apps/desktop/src-tauri/src/execution_targets.rs", "apps/desktop/src-tauri/src/execution_targets/agent_installs.rs",
  "apps/desktop/src-tauri/src/execution_targets/agent_updates.rs", "apps/desktop/src-tauri/src/execution_targets/agents.rs",
  "apps/desktop/src-tauri/src/terminal_host/remote.rs", "apps/desktop/src-tauri/src/daemon/remote.rs", "crates/antigravity-driver/src/approval_bridge.rs",
  "crates/agent-management/src/install.rs", "crates/agent-management/src/detect.rs", "crates/agent-management/src/operation.rs", "crates/agent-management/src/http.rs",
  "crates/agent-management/src/updates.rs", "crates/agent-management/src/updates/codex_host.rs", "crates/agent-management/src/updates/electron_host.rs",
  "crates/agent-management/src/updates/kiro.rs", "crates/agent-management/src/updates/managed_cli.rs", "crates/agent-management/src/updates/new_agents.rs",
];
const coded = (code: string, ...params: string[]) => params.length ? `${code}\u001e${JSON.stringify(params)}` : code;

describe("backend text localization", () => {
  it("leaves only separately translated Chinese in agent and remote sources", () => {
    const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
    const missing = REMOTE_SOURCES.flatMap(source => readFileSync(join(root, source), "utf8").split("#[cfg(test)]")[0].split("\n")
      .filter(line => !line.trim().startsWith("//"))
      .flatMap(line => [...line.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map(match => match[1]))
      .filter(text => /\p{Script=Han}/u.test(text))
      .map(text => `${source}: ${text}`));
    expect(missing).toEqual([]);
  });

  it("translates codes in every language, including zh-CN", () => {
    for (const lang of Object.keys(LOCALES) as ResolvedLanguage[]) {
      expect(localizeBackendText("phone.unconnected", lang)).toBeTruthy();
      expect(localizeBackendText(coded("backendText.installNodeTooOld", "v22.3.0"), lang)).toContain("v22.3.0");
      expect(localizeBackendText(coded("backendText.installFailedCode", "35"), lang)).toContain("35");
      expect(localizeBackendText(coded("backendText.agentRunsOneTaskAtATime", "Antigravity"), lang)).toContain("Antigravity");
    }
    expect(localizeBackendText("phone.unconnected", "zh-CN")).toBe("手机配对连接未完成，请核实或重新生成邀请。");
    expect(localizeBackendText("phone.unconnected", "en")).toBe("Phone pairing didn't finish. Check it or generate a new invitation.");
    expect(displayBackendText("phone.unconnected", "zh-CN")).not.toBe("phone.unconnected");
  });

  it("translates remote pairing, terminal and analysis failures from their codes", () => {
    expect(localizeBackendText(coded("phone.httpError", "503"), "en")).toContain("503");
    expect(localizeBackendText(coded("phone.connectionFailed", "timeout"), "ja")).toContain("timeout");
    expect(localizeBackendText(coded("shell.exitUnverified", "reaper unavailable"), "de")).toContain("reaper unavailable");
    expect(localizeBackendText("project.analysisInterrupted", "zh-CN")).toBe("项目分析已中断，请重新分析；未自动重新执行");
    expect(displayBackendText("not a code", "en")).toBe("not a code");
  });

  it("translates parameterized and agent-error-wrapped messages", () => {
    expect(localizeBackendText(coded("backendText.quotaExhaustedReset", "2026-01-02 03:04 UTC"), "en")).toContain("2026-01-02 03:04 UTC");
    const wrapped = localizeBackendText(coded("backendText.failureAgentErrorSuffix", "backendText.rateLimited", "429 too many"), "en");
    expect(wrapped).toContain("rate limited");
    expect(wrapped).toContain("429 too many");
  });

  it("translates undo errors, nested failures and recovery guidance in every language", () => {
    for (const lang of Object.keys(LOCALES) as ResolvedLanguage[]) {
      for (const message of ["backendText.undoIncompleteSnapshot", "backendText.undoIncompleteTurn"]) {
        const translated = localizeBackendText(message, lang);
        expect(translated).toBeTruthy();
        expect(translated).not.toBe(message);
      }
      const conflict = coded("backendText.undoFileChanged", "src/example.ts");
      const translated = localizeBackendText(conflict, lang)!;
      expect(translated).toContain("src/example.ts");
      for (const recovery of ["backendText.undoRolledBack", "backendText.undoBackupsRetained"]) {
        const wrapped = localizeBackendText(coded("backendText.undoIncomplete", conflict, recovery), lang)!;
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
      expect(userFacingError("backendText.attachmentUnreadable", "fallback")).toBe("The attachment can no longer be read.");
    } finally {
      setGlobalResolvedLanguage("zh-CN");
    }
    expect(userFacingError("某个没有映射的后端错误", "fallback")).toBe("某个没有映射的后端错误");
    expect(userFacingError("phone.unconnected", "fallback")).toBe("手机配对连接未完成，请核实或重新生成邀请。");
  });

  it("translates permission, missing-export and redacted push details", () => {
    expect(localizeBackendText("backendText.permissionModeUnconfirmed", "zh-CN")).toBe("Agent 未确认权限模式");
    expect(localizeBackendText("backendText.permissionModeUnconfirmed", "en")).toBe("The agent did not confirm the permission mode.");
    expect(displayBackendText("driver protocol error: backendText.permissionModeUnconfirmed", "en")).toBe("driver protocol error: The agent did not confirm the permission mode.");
    expect(localizeBackendText(coded("backendText.m1e65d465", coded("backendText.missingSymbol", "MFStartup")), "zh-CN")).toBe("Windows 系统视频编码不可用：缺少 MFStartup");
    expect(localizeBackendText(coded("backendText.m1e65d465", coded("backendText.missingSymbol", "MFStartup")), "en")).toContain("Missing MFStartup");
    const pushed = localizeBackendText(coded("backendText.m2b08f8cc", "0", "1", "fatal: backendText.pushAddress"), "zh-CN");
    expect(pushed).toContain("[推送地址]");
    expect(pushed).not.toContain("backendText.pushAddress");
  });
});
