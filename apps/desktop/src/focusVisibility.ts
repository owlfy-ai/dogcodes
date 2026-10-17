const navigationKeys = new Set(["Tab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "Enter", " "]);

/** Show control focus rings during navigation; dismiss them on pointer input or Escape. */
export function installFocusVisibility(document: Document) {
  const root = document.documentElement;
  const previous = root.getAttribute("data-focus-modality");
  root.dataset.focusModality = "keyboard";
  const pointer = () => { root.dataset.focusModality = "pointer"; };
  const keyboard = (event: KeyboardEvent) => {
    if (event.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
    // Clear keyboard highlighting before menus restore focus to their trigger.
    // Keep logical focus so the next Tab continues from the same control.
    if (event.key === "Escape") { pointer(); return; }
    // Typing in a search field must not highlight the return button.
    if (!navigationKeys.has(event.key)) return;
    const target = event.target;
    if (event.key !== "Tab" && target instanceof HTMLElement
      && (target.matches("input:not([type=checkbox]):not([type=radio]):not([type=button]):not([type=submit]):not([type=reset]), textarea") || target.isContentEditable)) {
      // ArrowDown commonly moves from a menu's search field into its options.
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    }
    root.dataset.focusModality = "keyboard";
  };
  // Capture before menu handlers restore focus, even when they stop propagation.
  document.addEventListener("pointerdown", pointer, true);
  document.addEventListener("keydown", keyboard, true);
  return () => {
    document.removeEventListener("pointerdown", pointer, true);
    document.removeEventListener("keydown", keyboard, true);
    if (previous === null) root.removeAttribute("data-focus-modality");
    else root.setAttribute("data-focus-modality", previous);
  };
}
