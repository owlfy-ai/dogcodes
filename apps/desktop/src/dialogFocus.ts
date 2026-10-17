/** Start dialogs on their heading; Tab then reaches the normal controls. */
export function focusDialogTitle(dialog: HTMLElement | null) {
  dialog?.querySelector<HTMLElement>("[data-dialog-title]")?.focus({ preventScroll: true });
}
