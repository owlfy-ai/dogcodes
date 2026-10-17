// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it } from "vitest";
import { installFocusVisibility } from "./focusVisibility";

let dispose: () => void;
const mode = () => document.documentElement.dataset.focusModality;
const pointer = (target: EventTarget = document.body) => target.dispatchEvent(new Event("pointerdown", { bubbles: true }));
const key = (key: string, target: EventTarget = document.body, options: KeyboardEventInit = {}) =>
  target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, ...options }));

beforeEach(() => { dispose = installFocusVisibility(document); });
afterEach(() => { dispose(); document.body.replaceChildren(); });

it("keeps pointer mode when Escape restores focus to an icon", () => {
  const button = document.createElement("button");
  document.body.append(button);
  pointer(button);
  button.addEventListener("keydown", event => { event.stopPropagation(); button.focus(); });
  key("Escape", button);
  expect(document.activeElement).toBe(button);
  expect(mode()).toBe("pointer");
});

it("dismisses Tab highlighting with Escape and restores it on the next Tab", () => {
  const button = document.createElement("button");
  document.body.append(button);
  pointer();
  key("Tab");
  button.focus();
  expect(mode()).toBe("keyboard");
  key("Escape", button);
  expect(mode()).toBe("pointer");
  expect(document.activeElement).toBe(button);
  key("Tab", button);
  expect(mode()).toBe("keyboard");
  pointer();
  expect(mode()).toBe("pointer");
});

it("dismisses keyboard highlighting before an Escape handler restores menu-trigger focus", () => {
  const trigger = document.createElement("button");
  const option = document.createElement("button");
  document.body.append(trigger, option);
  key("ArrowDown"); option.focus();
  let observed: string | undefined;
  option.addEventListener("keydown", event => {
    observed = mode(); event.stopPropagation(); trigger.focus();
  });
  key("Escape", option);
  expect(observed).toBe("pointer");
  expect(document.activeElement).toBe(trigger);
  expect(mode()).toBe("pointer");
});

it.each(["ArrowDown", "ArrowUp", "Home", "End", "Enter", " "])("enables focus feedback before a control handles %s", navigationKey => {
  const button = document.createElement("button");
  document.body.append(button);
  pointer(button);
  let observed: string | undefined;
  button.addEventListener("keydown", event => { observed = mode(); event.stopPropagation(); });
  key(navigationKey, button);
  expect(observed).toBe("keyboard");
});

it("does not turn search typing or shortcuts into navigation, but allows moving into menu options", () => {
  const input = document.createElement("input");
  document.body.append(input);
  pointer(input); input.focus();
  for (const value of ["a", " ", "ArrowLeft", "Home", "Enter", "Escape", "Shift"]) key(value, input);
  key("Home", input, { metaKey: true });
  key("ArrowDown", input, { isComposing: true });
  expect(mode()).toBe("pointer");
  key("ArrowDown", input);
  expect(mode()).toBe("keyboard");
});

it("removes listeners and its document attribute on disposal", () => {
  dispose();
  pointer(); key("Tab");
  expect(mode()).toBeUndefined();
});
