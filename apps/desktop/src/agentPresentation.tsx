import type { CSSProperties } from "react";
import chatgptSvg from "./assets/chatgpt.svg?raw";
import cursorSvg from "./assets/cursor.svg?raw";
import grokSvg from "./assets/grok.svg?raw";
import zcodeSvg from "./assets/zcode.svg?raw";

const antigravityLogo = new URL("./assets/antigravity.svg", import.meta.url).href;
const claudeCodeLogo = new URL("./assets/claude-code.svg", import.meta.url).href;
const codebuddyLogo = new URL("./assets/codebuddy.svg", import.meta.url).href;
const kimiLogo = new URL("./assets/kimi.svg", import.meta.url).href;

// Keep artwork and frame in one replaced element. Theme-specific SVG sources
// avoid nested artwork layers and filter-related raster offsets in popovers.
const monochromeLogos: Record<string, { dark: string; light: string }> = Object.fromEntries(
  Object.entries({ codex: chatgptSvg, cursor: cursorSvg, grok: grokSvg, zcode: zcodeSvg })
    .map(([providerId, svg]) => [providerId, {
      dark: `data:image/svg+xml,${encodeURIComponent(svg)}`,
      light: `data:image/svg+xml,${encodeURIComponent(svg.replace(/fill="#[^"]+"/g, 'fill="#000"'))}`,
    }]),
);
const logos: Record<string, string> = {
  antigravity: antigravityLogo,
  "claude-code": claudeCodeLogo,
  codebuddy: codebuddyLogo,
  kimi: kimiLogo,
  pi: new URL("./assets/pi.svg", import.meta.url).href,
  opencode: new URL("./assets/opencode.png", import.meta.url).href,
  "deepseek-harness": new URL("./assets/deepseek-harness.png", import.meta.url).href,
  kiro: new URL("./assets/kiro.svg", import.meta.url).href,
};

export function providerLabel(providerId: string): string {
  const labels: Record<string, string> = {
    codex: "Codex",
    "claude-code": "Claude Code",
    kimi: "Kimi",
    cursor: "Cursor",
    codebuddy: "CodeBuddy",
    antigravity: "Antigravity",
    grok: "Grok Build",
    zcode: "ZCode",
    pi: "Pi",
    opencode: "OpenCode",
    "deepseek-harness": "DeepSeek Harness",
    kiro: "Kiro",
    fake: "离线模式",
    "fake-peer": "离线协作",
  };
  return labels[providerId] ?? providerId;
}

export function AgentIcon({ providerId, size = "medium" }: { providerId: string; size?: "small" | "medium" }) {
  const normalized = providerId.toLowerCase();
  const monochrome = monochromeLogos[normalized];
  const src = monochrome?.dark ?? logos[normalized];
  if (src) {
    return <img className={`agent-icon ${size} agent-${normalized}${monochrome ? " agent-icon-monochrome" : ""}`} src={src}
      style={monochrome ? { "--agent-logo-light": `url("${monochrome.light}")` } as CSSProperties : undefined}
      data-tooltip={providerLabel(normalized)} alt="" />;
  }
  const glyph = normalized.startsWith("fake") ? "F" : providerId.slice(0, 1).toUpperCase();
  return <span className={`agent-icon ${size} agent-${normalized}`} data-tooltip={providerLabel(providerId)} aria-hidden="true">{glyph}</span>;
}
