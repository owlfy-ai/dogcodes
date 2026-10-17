import { readFileSync } from "node:fs";
import { JSDOM } from "jsdom";
import { describe, expect, it } from "vitest";

describe("agent quota layout", () => {
  it("keeps the ring and percentage horizontal inside the menu with room for 100%", () => {
    const dom = new JSDOM(`<style>${["styles.css", "agentPicker.css"].map(file => readFileSync(new URL(file, import.meta.url), "utf8")).join("\n")}</style>
      <div class="agent-picker-menu" data-agent-picker><button class="agent-picker-option"><span class="agent-icon"></span>
      <span class="agent-option-label"><b>Codex</b></span>
      <span class="agent-option-quota"><svg></svg><span class="quota-ring-value">100%</span></span><i>✓</i>
      </button></div>`);
    const { window } = dom;
    const indicator = window.getComputedStyle(window.document.querySelector(".agent-option-quota")!);
    expect(indicator.display).toBe("inline-flex");
    expect(indicator.whiteSpace).toBe("nowrap");
    expect(window.getComputedStyle(window.document.querySelector("button")!).gridTemplateColumns)
      .toBe("calc(28px * var(--ui-scale, 1)) minmax(0, 1fr) calc(58px * var(--ui-scale, 1)) 16px");
    dom.window.close();
  });
});
