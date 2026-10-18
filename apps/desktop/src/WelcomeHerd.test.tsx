// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { herdLayout, WelcomeHerd, type HerdTone } from "./WelcomeHerd";

let root: Root | undefined;
let container: HTMLDivElement;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.useFakeTimers();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = undefined;
  container.remove();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const ids = Array.from({ length: 12 }, (_, index) => `agent-${index}`);
const agents = (ready: string[], scanning = true) =>
  ids.map(id => ({ id, tone: (ready.includes(id) ? "installed" : scanning ? "scanning" : "missing") as HerdTone }));
const render = (ready: string[], scanning = true) =>
  act(async () => root!.render(<WelcomeHerd agents={agents(ready, scanning)} renderIcon={id => <i data-icon={id} />} />));
const alerted = () => container.querySelector<HTMLElement>(".welcome-herd-dog")!.hasAttribute("data-alert");

describe("welcome herd", () => {
  it("keeps one fixed slot per agent and keeps the dog behind the flock", () => {
    const { slots, size, dog } = herdLayout(12);
    expect(new Set(slots.map(slot => slot.join())).size).toBe(12);
    for (const [x] of slots) expect(x - size / 2).toBeGreaterThan(dog[0] + .07);
  });

  it("shows unknown agents as question-mark sheep and lights them up in place with their logo", async () => {
    await render([]);
    expect(container.querySelectorAll(".welcome-sheep")).toHaveLength(12);
    expect(container.querySelectorAll(".welcome-sheep.unknown .welcome-sheep-unknown")).toHaveLength(12);
    const before = container.querySelector<HTMLElement>('[data-agent="agent-5"]')!.getAttribute("style");

    await render(["agent-5"]);
    const found = container.querySelector<HTMLElement>('[data-agent="agent-5"]')!;
    expect(found.className).toContain("ready");
    expect(found.querySelector('[data-icon="agent-5"]')).not.toBeNull();
    expect(found.getAttribute("style")).toBe(before);
  });

  it("drives the flock from behind and looks up when an agent is found", async () => {
    await render(["agent-0"]);
    // Agents already found when the page opens are not announced.
    expect(alerted()).toBe(false);

    await render(["agent-0", "agent-7"]);
    expect(alerted()).toBe(true);
    await act(async () => vi.advanceTimersByTime(1500));
    expect(alerted()).toBe(false);

    // Detection ending changes nothing: the herd keeps walking.
    await render(["agent-0", "agent-7"], false);
    expect(alerted()).toBe(false);
  });
});
