// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { herdLayout, WelcomeHerd, type HerdTone } from "./WelcomeHerd";

let root: Root | undefined;
let container: HTMLDivElement;
let clock = 0;
let frames: FrameRequestCallback[] = [];

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  clock = 0;
  frames = [];
  vi.spyOn(performance, "now").mockImplementation(() => clock);
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => frames.push(callback));
  vi.stubGlobal("cancelAnimationFrame", () => { frames = []; });
});
afterEach(async () => {
  if (root) await act(async () => root?.unmount());
  root = undefined;
  container?.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const ids = Array.from({ length: 12 }, (_, index) => `agent-${index}`);
const agents = (ready: string[]) => ids.map(id => ({ id, tone: (ready.includes(id) ? "installed" : "scanning") as HerdTone }));
const render = (ready: string[]) => act(async () => root!.render(<WelcomeHerd agents={agents(ready)} renderIcon={id => <i data-icon={id} />} />));
const advance = (seconds: number) => {
  for (let step = 0; step < seconds * 60; step++) {
    clock += 1000 / 60;
    const pending = frames;
    frames = [];
    pending.forEach(callback => callback(clock));
  }
};
const dog = () => container.querySelector<HTMLElement>(".welcome-herd-dog")!;
const position = () => [Number(dog().style.getPropertyValue("--x")), Number(dog().style.getPropertyValue("--y"))];

describe("welcome herd", () => {
  it("keeps one fixed slot per agent and a dog loop clear of every sheep", () => {
    const { slots, size, loop } = herdLayout(12);
    expect(new Set(slots.map(slot => slot.join())).size).toBe(12);
    for (const [x, y] of slots) {
      expect(Math.abs(x) + size / 2).toBeLessThan(loop.a - .07);
      expect(y).toBeGreaterThan(loop.cy - loop.back);
      expect(y).toBeLessThan(loop.cy + loop.front - .1);
    }
  });

  it("shows unknown agents as question-mark sheep and lights them up with their logo once found", async () => {
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    await render([]);
    expect(container.querySelectorAll(".welcome-sheep")).toHaveLength(12);
    expect(container.querySelectorAll(".welcome-sheep.unknown .welcome-sheep-unknown")).toHaveLength(12);
    const slot = container.querySelector<HTMLElement>('[data-agent="agent-5"]')!;
    const place = [slot.dataset.x, slot.dataset.y];

    await render(["agent-5"]);
    const found = container.querySelector<HTMLElement>('[data-agent="agent-5"]')!;
    expect(found.className).toContain("ready");
    expect(found.querySelector('[data-icon="agent-5"]')).not.toBeNull();
    // Nothing reflows when an agent is found.
    expect([found.dataset.x, found.dataset.y]).toEqual(place);
  });

  it("trots around the flock to the newly found agent and stops to eye it", async () => {
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    await render([]);
    const { slots, loop } = herdLayout(12);
    let largestStep = 0;
    let last = position();
    await render(["agent-11"]);
    let stopped = false;
    for (let second = 0; second < 40 && !stopped; second++) {
      for (let tick = 0; tick < 10; tick++) {
        advance(.1);
        const next = position();
        largestStep = Math.max(largestStep, Math.hypot(next[0] - last[0], next[1] - last[1]));
        last = next;
        // The dog stays on its loop, outside the flock, the whole way.
        const [x, y] = next;
        const semi = y < loop.cy ? loop.back : loop.front;
        expect(Math.hypot(x / loop.a, (y - loop.cy) / semi)).toBeCloseTo(1, 2);
        if (dog().dataset.gait === "eye") { stopped = true; break; }
      }
    }
    expect(stopped).toBe(true);
    // No jumps: at most a trot's worth of distance per tenth of a second.
    expect(largestStep).toBeLessThan(.012);
    const [sx] = slots[11];
    const [x] = position();
    expect(Math.sign(x)).toBe(Math.sign(sx));
  });
});
