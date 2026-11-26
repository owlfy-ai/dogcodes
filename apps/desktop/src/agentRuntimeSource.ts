import type { InstallationSummary } from "@dogcodes/protocol";

const cliOnlyAgents = new Set(["cursor", "antigravity", "codebuddy", "kimi", "grok", "pi", "kiro"]);
export const agentRequiresCli = (providerId: string): boolean => cliOnlyAgents.has(providerId);

// Classify the selected executable, not the app supplying its credentials.
export function agentRuntimeSource(installation: Pick<InstallationSummary, "providerId" | "executable">): "desktop" | "cli" | null {
  if (!installation.executable || installation.providerId === "fake") return null;
  const path = installation.executable.replaceAll("\\", "/").toLowerCase();
  let desktop = false;
  switch (installation.providerId) {
    case "codex":
      desktop = /\/(?:chatgpt|codex)\.app\/contents\/resources\/(?:codex-cli\/bin\/)?codex$/.test(path)
        || /\/openai\/codex\/bin\/[^/]+\/codex\.exe$/.test(path);
      break;
    case "claude-code":
      desktop = /\/(?:claude|claude-data\/[^/]+)\/claude-code\/[^/]+\/[^/]+\/(?:claude\.app\/contents\/macos\/claude|claude\.exe)$/.test(path);
      break;
    case "zcode":
      desktop = /\/zcode\.app\/contents\/macos\/zcode$/.test(path) || /\/zcode\/zcode\.exe$/.test(path);
      break;
    case "qoder-cn":
      // Only the desktop app ships this CLI, as a script its Electron binary runs.
      desktop = /\/qoder cn\.app\/contents\/macos\/qoder cn$/.test(path) || /\/qoder cn\/qoder cn\.exe$/.test(path);
      break;
    case "deepseek-harness":
      desktop = /\/deepseek harness(?:\.app\/contents)?\/resources\/runtime\/cli\/bin\/dsh(?:\.cmd)?$/.test(path);
      break;
    case "opencode":
      desktop = /\/opencode\.app\/contents\/resources\/opencode-cli$/.test(path)
        || /\/ai\.opencode\.desktop\/(?:resources|cli\/[^/]+)\/opencode-cli(?:\.exe)?$/.test(path);
      break;
  }
  return desktop ? "desktop" : "cli";
}
