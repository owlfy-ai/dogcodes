import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const readSource = (relativePath: string) =>
  readFileSync(new URL(relativePath, import.meta.url), "utf8");

describe("window chrome", () => {
  it("scales visual elements without scaling the viewport layout", () => {
    const css = readSource("./styles.css");
    const shell = css.match(/\.app-shell\s*\{([^}]+)\}/)?.[1];
    expect(shell).not.toContain("zoom:");
    expect(shell).not.toContain("transform:");
    expect(shell).toContain("height: 100vh");
    expect(shell).toContain("width: 100%");
    expect(shell).not.toContain("var(--ui-scale)");
    expect(css).not.toMatch(/(?:^|[;{])\s*font-size:\s*\d+(?:\.\d+)?px/);
    expect(css.match(/\.agent-icon\.small\s*\{([^}]+)\}/)?.[1])
      .toContain("width: calc(20px * var(--ui-scale))");
  });
  it("grants the native window-drag command to the main window", () => {
    const capability = JSON.parse(
      readSource("../src-tauri/capabilities/default.json"),
    ) as { permissions: string[] };

    expect(capability.permissions).toContain("core:window:allow-start-dragging");
    const detached = JSON.parse(readSource("../src-tauri/capabilities/review.json"));
    expect(detached.windows).toContain("review-*");
    expect(detached.permissions).toContain("core:window:allow-start-dragging");
  });

  it("makes every app title bar a deep drag region", () => {
    const titleBarSources = [
      readSource("./App.tsx"),
      readSource("./ReviewPanel.tsx"),
      readSource("./StartupGate.tsx"),
      readSource("./RecoveryCenter.tsx"),
    ].join("\n");

    expect(titleBarSources).not.toMatch(/data-tauri-drag-region(?:\s|>)/);
    const titleBars = titleBarSources.match(/<header\b[^>]*className="(?:sidebar-titlebar|task-header|review-titlebar)"[^>]*>/g) ?? [];
    expect(titleBars.length).toBeGreaterThan(0);
    for (const titleBar of titleBars) expect(titleBar).toContain('data-tauri-drag-region="deep"');
  });
});
