import { describe, expect, it } from "vitest";
import { agentRuntimeSource } from "./agentRuntimeSource";

describe("selected agent runtime source", () => {
  it.each([
    ["codex", "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex"],
    ["codex", "/Users/me/Applications/Codex.app/Contents/Resources/codex"],
    ["codex", "C:\\Users\\中文 用户\\AppData\\Local\\OpenAI\\Codex\\bin\\hash\\codex.exe"],
    ["claude-code", "/Users/me/Library/Application Support/Claude/claude-code/2.1.325/hash/claude.app/Contents/MacOS/claude"],
    ["claude-code", "C:/Users/me/AppData/Local/Packages/Claude_x/LocalCache/Roaming/Claude/claude-code/2.1.325/hash/claude.exe"],
    ["claude-code", "C:\\Users\\me\\AppData\\Local\\Claude-Data\\claude_x\\claude-code\\2.1.330\\hash\\claude.exe"],
    ["claude-code", "C:\\Users\\me\\AppData\\Local\\Claude-Data\\claude_x\\claude-code\\2.1.330\\hash\\claude.exe"],
    ["zcode", "/Applications/ZCode.app/Contents/MacOS/ZCode"],
    ["zcode", "C:/Users/me/AppData/Local/Programs/ZCode/ZCode.exe"],
    ["deepseek-harness", "/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh"],
    ["deepseek-harness", "C:/Users/me/AppData/Local/Programs/DeepSeek Harness/resources/runtime/cli/bin/dsh.cmd"],
    ["opencode", "/Applications/OpenCode.app/Contents/Resources/opencode-cli"],
    ["opencode", "/Users/me/Library/Application Support/ai.opencode.desktop/cli/2.2.6-arm64/opencode-cli"],
    ["opencode", "C:/Users/me/AppData/Local/Programs/ai.opencode.desktop/resources/opencode-cli.exe"],
    ["opencode", "C:/Users/me/AppData/Roaming/ai.opencode.desktop/cli/2.2.6-x64/opencode-cli.exe"],
  ])("identifies %s's desktop runtime at %s", (providerId, executable) => {
    expect(agentRuntimeSource({ providerId, executable })).toBe("desktop");
  });
  it.each([
    ["claude-code", "/Users/me/.local/bin/claude"], // Desktop credentials do not change this.
    ["codex", "/opt/homebrew/bin/codex"],
    ["cursor", "C:/Users/me/AppData/Local/cursor-agent/cursor-agent.cmd"],
    ["kiro", "/Applications/Kiro CLI.app/Contents/MacOS/kiro-cli"],
    ["kimi", "/Users/me/.kimi-code/bin/kimi"],
    ["antigravity", "/Users/me/.local/bin/agy"],
    ["codebuddy", "/usr/local/bin/codebuddy"],
    ["pi", "/Users/me/.pi/agent/bin/pi"],
    ["grok", "/Users/me/.grok/bin/grok"],
    ["deepseek-harness", "/Users/me/.local/share/dogcodes/cli/deepseek-harness/lib/node_modules/@deepseek-ai/dsh/lib/bin.js"],
    ["opencode", "/Users/me/.opencode/bin/opencode"],
  ])("keeps %s's standalone executable classified as CLI", (providerId, executable) => {
    expect(agentRuntimeSource({ providerId, executable })).toBe("cli");
  });
  it("does not label missing installations or the demo agent", () => {
    expect(agentRuntimeSource({ providerId: "codex", executable: "" })).toBeNull();
    expect(agentRuntimeSource({ providerId: "fake", executable: "/bin/fake" })).toBeNull();
  });
});
