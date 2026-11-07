import { readFileSync } from "node:fs";
import { JSDOM } from "jsdom";
import { describe, expect, it } from "vitest";

describe("composer permission label", () => {
  const visible = (stage: number, marker = "") => {
    const dom = new JSDOM(`<style>${readFileSync(new URL("styles.css", import.meta.url), "utf8")}</style>
      <form class="composer"><footer class="composer-controls" data-label-stage="${stage}">
        <button class="permission-trigger ${marker}"><span class="composer-control-label">Skip</span></button>
      </footer></form>`);
    const label = dom.window.document.querySelector(".composer-control-label")!;
    const shown = dom.window.getComputedStyle(label).display !== "none";
    dom.window.close();
    return shown;
  };

  it("is the first to go for an ordinary mode", () => {
    expect([0, 1, 2, 3, 4, 5].map(stage => visible(stage))).toEqual([true, true, false, false, false, false]);
  });

  it.each(["dangerous", "caution"])("is the last to go for a %s mode", marker => {
    expect([0, 1, 2, 3, 4, 5].map(stage => visible(stage, marker))).toEqual([true, true, true, true, true, false]);
  });
});

describe("agent quota layout", () => {
  it("keeps the composer agent name beside a quota block that shrinks when there is no bar", () => {
    const dom = new JSDOM(`<style>${["styles.css", "agentPicker.css"].map(file => readFileSync(new URL(file, import.meta.url), "utf8")).join("\n")}</style>
      <footer class="composer-controls" data-label-stage="1">
        <details class="agent-picker"><summary>
          <b class="composer-control-label">DeepSeek Harness</b>
          <span class="agent-option-quota"><span class="picker-quota-amount">CNY 302.59</span></span>
        </summary></details>
        <details class="agent-picker"><summary>
          <b class="composer-control-label">Codex</b>
          <span class="agent-option-quota"><span class="picker-quota-label">5 hours</span><span class="picker-quota-bar"></span><span class="picker-quota-value">100%</span></span>
        </summary></details>
      </footer>`);
    const { window } = dom;
    const [balance, bars] = [...window.document.querySelectorAll<HTMLElement>(".agent-option-quota")];
    const name = window.getComputedStyle(window.document.querySelector(".composer-control-label")!);
    expect(name.display).not.toBe("none");
    expect(name.overflow).toBe("visible");
    expect(window.getComputedStyle(balance).gridTemplateColumns).toBe("max-content");
    expect(window.getComputedStyle(bars).gridTemplateColumns).toBe("max-content 64px max-content");
    expect(window.document.querySelector(".dropdown-chevron")).toBeNull();
    dom.window.close();
  });
});
