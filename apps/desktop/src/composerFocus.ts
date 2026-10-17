/** Return focus to the prompt belonging to the control that was just selected. */
export function focusComposerInput(control: HTMLElement | null): boolean {
  const input = control?.closest("form.composer")?.querySelector<HTMLTextAreaElement>("textarea");
  if (!input || !input.isConnected || input.disabled) return false;
  input.focus({ preventScroll: true });
  return true;
}
