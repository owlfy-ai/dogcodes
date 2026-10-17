import { describe, expect, it } from "vitest";
import { adjustUiScale, parseUiScale, scaleShortcut } from "./uiScale";

describe("visual size controls", () => {
  it("validates numeric percentages and rejects obsolete named sizes", () => {
    expect(["small", "default", "large"].map(parseUiScale)).toEqual([100, 100, 100]);
    expect(parseUiScale("130")).toBe(130);
    expect(parseUiScale("invalid")).toBe(100);
    expect(parseUiScale(null)).toBe(100);
    expect(parseUiScale("999")).toBe(150);
  });
  it("steps repeatedly, clamps the bounds and resets", () => {
    expect(adjustUiScale(adjustUiScale(110, "in"), "in")).toBe(130);
    expect(adjustUiScale(150, "in")).toBe(150);
    expect(adjustUiScale(70, "out")).toBe(70);
    expect(adjustUiScale(130, "reset")).toBe(100);
  });
  it("supports command/control shortcuts without intercepting ordinary typing or IME", () => {
    const event = { key: "=", metaKey: true, ctrlKey: false, altKey: false, isComposing: false };
    expect(scaleShortcut(event)).toBe("in");
    expect(scaleShortcut({ ...event, key: "+" })).toBe("in");
    expect(scaleShortcut({ ...event, key: "-", metaKey: false, ctrlKey: true })).toBe("out");
    expect(scaleShortcut({ ...event, key: "0" })).toBe("reset");
    expect(scaleShortcut({ ...event, metaKey: false })).toBeNull();
    expect(scaleShortcut({ ...event, isComposing: true })).toBeNull();
    expect(scaleShortcut({ ...event, altKey: true })).toBeNull();
  });
});
