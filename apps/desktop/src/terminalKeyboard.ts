import type { Terminal } from "@xterm/xterm";

/** WebKit may use keyCode 229 for ordinary English and overlapping keystrokes.
 * Handle committed insertText directly instead of xterm's delayed textarea diff.
 * Native composition, paste, control keys and terminal escape sequences stay in xterm.
 */
export function installTerminalKeyboard(terminal: Terminal, host: HTMLElement, webkit = /AppleWebKit/.test(navigator.userAgent) && !/(Chrome|Chromium|Edg)\//.test(navigator.userAgent)) {
  if (!webkit) return () => {};
  let composing = false, emitted = false, keyPending = false;
  const sent = terminal.onData(() => { if (keyPending) emitted = true; });
  const keydown = (event: KeyboardEvent) => {
    if (event.target !== terminal.textarea) return;
    if (["Shift", "Control", "Alt", "Meta", "CapsLock"].includes(event.key)) return;
    keyPending = true; emitted = false;
    if (event.keyCode === 229 && !composing && !event.isComposing) {
      // Keep the browser default so input/composition events still fire, but
      // don't schedule xterm's asynchronous whole-textarea diff for this key.
      event.stopImmediatePropagation();
    }
  };
  const input = (event: Event) => {
    const value = event as InputEvent;
    if (event.target !== terminal.textarea || composing || value.isComposing
      || value.inputType !== "insertText" || !value.data) return;
    event.stopImmediatePropagation();
    if (!keyPending || !emitted) terminal.input(value.data, true);
    // insertText is fully consumed; prevent old committed text accumulating.
    if (terminal.textarea) terminal.textarea.value = "";
    keyPending = false; emitted = false;
  };
  const start = () => { composing = true; keyPending = false; emitted = false; };
  const end = () => { composing = false; };
  const reset = () => { keyPending = false; emitted = false; };
  host.addEventListener("keydown", keydown, true);
  host.addEventListener("keyup", reset, true);
  host.addEventListener("input", input, true);
  host.addEventListener("compositionstart", start, true);
  host.addEventListener("compositionend", end, true);
  host.addEventListener("blur", reset, true);
  return () => {
    sent.dispose();
    host.removeEventListener("keydown", keydown, true);
    host.removeEventListener("keyup", reset, true);
    host.removeEventListener("input", input, true);
    host.removeEventListener("compositionstart", start, true);
    host.removeEventListener("compositionend", end, true);
    host.removeEventListener("blur", reset, true);
  };
}
