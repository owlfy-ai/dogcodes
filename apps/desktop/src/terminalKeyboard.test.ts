// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { installTerminalKeyboard } from "./terminalKeyboard";
import type { Terminal } from "@xterm/xterm";
let terminal: Terminal, host: HTMLDivElement, data: string[], cleanup: () => void;
beforeEach(async () => {
  vi.useFakeTimers();
  vi.stubGlobal("matchMedia", () => ({ matches: false, addListener() {}, removeListener() {}, addEventListener() {}, removeEventListener() {} }));
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  // Actual xterm event processing, not the TerminalDock mock.
  const { Terminal } = await import("@xterm/xterm");
  terminal = new Terminal({ allowProposedApi: true });
  host = document.createElement("div"); document.body.append(host);
  terminal.open(host); data = []; terminal.onData(text => data.push(text));
  cleanup = () => {};
});
afterEach(() => { cleanup(); terminal.dispose(); host.remove(); vi.useRealTimers(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
function key(text: string) {
  terminal.textarea!.dispatchEvent(new KeyboardEvent("keydown", { key: text, keyCode: 229, bubbles: true, cancelable: true }));
  terminal.textarea!.value += text;
  terminal.textarea!.dispatchEvent(new InputEvent("input", { inputType: "insertText", data: text, bubbles: true, composed: true }));
}
it("reproduces xterm losing a rapid repeated character before the write queue", () => {
  key("a"); key("a"); vi.runOnlyPendingTimers();
  // The two delayed diff callbacks see the same textarea value and corrupt it.
  expect(data.join("")).not.toBe("aa");
});
it("preserves overlapping English input, repeated keys and shifted punctuation", () => {
  cleanup = installTerminalKeyboard(terminal, host, true);
  const text = "git status --short && echo \"hello\" /foo_bar 112233";
  for (const letter of text) key(letter);
  vi.runOnlyPendingTimers();
  expect(data.join("")).toBe(text);
});
it("does not duplicate ordinary keydown input and keeps control keys in xterm", () => {
  cleanup = installTerminalKeyboard(terminal, host, true);
  const area = terminal.textarea!;
  area.dispatchEvent(new KeyboardEvent("keydown", { key: "a", keyCode: 65, bubbles: true, cancelable: true }));
  area.dispatchEvent(new InputEvent("input", { inputType: "insertText", data: "a", bubbles: true, composed: true }));
  area.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", keyCode: 13, bubbles: true, cancelable: true }));
  expect(data.join("")).toBe("a\r");
});
it("leaves Chinese composition commits to xterm", () => {
  cleanup = installTerminalKeyboard(terminal, host, true);
  const area = terminal.textarea!;
  area.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  area.value = "中文";
  area.dispatchEvent(new CompositionEvent("compositionupdate", { data: "中文", bubbles: true }));
  area.dispatchEvent(new InputEvent("input", { inputType: "insertCompositionText", data: "中文", isComposing: true, bubbles: true }));
  vi.runOnlyPendingTimers();
  area.dispatchEvent(new CompositionEvent("compositionend", { data: "中文", bubbles: true }));
  vi.runOnlyPendingTimers();
  expect(data.join("")).toBe("中文");
});

function inputBeforeKeydown(text: string) {
  for (const letter of text) {
    terminal.textarea!.value += letter;
    terminal.textarea!.dispatchEvent(new InputEvent("input", { inputType: "insertText", data: letter, bubbles: true, composed: true }));
    terminal.textarea!.dispatchEvent(new KeyboardEvent("keydown", { key: letter, keyCode: 229, bubbles: true, cancelable: true }));
    // No keyup between characters: the physical key presses overlap.
    vi.runOnlyPendingTimers();
  }
}
it("reproduces missing second English character with WebKit input-before-keydown ordering", () => {
  inputBeforeKeydown("mo");
  expect(data.join("")).toBe("m");
});
it("keeps every character with WebKit input-before-keydown ordering", () => {
  cleanup = installTerminalKeyboard(terminal, host, true);
  inputBeforeKeydown("mo git status ll 112233");
  expect(data.join("")).toBe("mo git status ll 112233");
});
