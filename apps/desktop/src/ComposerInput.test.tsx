// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { ComposerInput } from "./App";

it("sends on Enter, preserves IME and Shift, and inserts a newline on Ctrl+Enter", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("requestAnimationFrame", (callback: () => void) => { callback(); return 0; });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const submit = vi.fn();
  const change = vi.fn();
  try {
    await act(async () => root.render(<form onSubmit={e => { e.preventDefault(); submit(); }}><ComposerInput draft="abcd" setDraft={change} /></form>));
    const input = container.querySelector("textarea")!;
    const press = (options: KeyboardEventInit = {}) => {
      const event = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true, ...options });
      input.dispatchEvent(event);
      return event;
    };
    expect(press({ shiftKey: true }).defaultPrevented).toBe(false);
    press({ isComposing: true });
    press({ keyCode: 229 });
    expect(submit).not.toHaveBeenCalled();
    input.setSelectionRange(1, 3);
    expect(press({ ctrlKey: true }).defaultPrevented).toBe(true);
    expect(change).toHaveBeenCalledWith("a\nd");
    expect(submit).not.toHaveBeenCalled();
    press();
    expect(submit).toHaveBeenCalledTimes(1);
    press({ repeat: true });
    expect(submit).toHaveBeenCalledTimes(1);
  } finally {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  }
});
