import { useEffect, useId, useRef, type ButtonHTMLAttributes, type FormEvent, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { focusDialogTitle } from "./dialogFocus";
import { translate as t } from "./i18n";
import "./appDialog.css";

const ICONS = {
  cancel: "M6 6l12 12M18 6 6 18",
  check: "m5 12 4.5 4.5L19 7",
  stop: "M7 7h10v10H7z",
  trash: "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v5m4-5v5",
  undo: "M9 5 4 10l5 5M4 10h10a5 5 0 0 1 0 10h-2",
  refresh: "M20 7v5h-5M4 17v-5h5M6.1 7a7 7 0 0 1 11.6-1L20 9M4 15l2.3 3A7 7 0 0 0 17.9 17",
  upgrade: "M12 16V4m-4 4 4-4 4 4M5 14v5h14v-5",
} as const;
export type DialogIcon = keyof typeof ICONS;
export type DialogTone = "default" | "primary" | "danger";

export function DialogButton({ tone = "default", icon, children, className = "", ...button }: ButtonHTMLAttributes<HTMLButtonElement> & { tone?: DialogTone; icon?: DialogIcon }) {
  return <button type="button" {...button} className={`app-dialog-button app-dialog-button--${tone} ${className}`.trim()}>
    {icon && <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={ICONS[icon]} /></svg>}
    {children}
  </button>;
}

/**
 * Every small modal in the app: opening, Escape, focus and returning focus live here.
 * It starts on the heading so a screen reader reads it first, unless an element asks
 * for focus with `data-dialog-default`, such as a field the person must fill in.
 */
export function AppDialog({ title, children, actions, onCancel, onSubmit, wide = false, returnFocus }: {
  title: ReactNode; children?: ReactNode; actions: ReactNode; onCancel: () => void;
  /** Makes Enter submit, with the actions inside the form. */
  onSubmit?: () => void; wide?: boolean;
  /** Where focus goes afterwards. WebKit doesn't focus a clicked button, so it can't be assumed. */
  returnFocus?: RefObject<HTMLElement | null>;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    const node = dialog.current;
    const previous = document.activeElement as HTMLElement | null;
    node?.showModal();
    const preferred = node?.querySelector<HTMLElement>("[data-dialog-default]");
    if (preferred) preferred.focus(); else focusDialogTitle(node);
    return () => {
      node?.close();
      (returnFocus?.current ?? (previous?.isConnected ? previous : null))?.focus({ preventScroll: true });
    };
  }, [returnFocus]);
  const content = <>
    <h2 id={titleId} tabIndex={-1} data-dialog-title>{title}</h2>
    {children && <div className="app-dialog-body">{children}</div>}
    <footer>{actions}</footer>
  </>;
  return createPortal(<dialog ref={dialog} className={`app-dialog${wide ? " app-dialog--wide" : ""}`} aria-labelledby={titleId}
    // A portal still passes clicks to the React parent, such as a row that would open.
    onClick={event => event.stopPropagation()}
    onCancel={event => { event.preventDefault(); onCancel(); }}>
    {onSubmit ? <form onSubmit={(event: FormEvent) => { event.preventDefault(); onSubmit(); }}>{content}</form> : content}
  </dialog>, document.body);
}

/** Asks one question. Primary is highlighted; use danger when it can't be taken back. */
export function ConfirmDialog({ title, children, confirmLabel, cancelLabel, icon = "check", tone = "primary", busy = false, disabled = false, wide, focusConfirm = false, returnFocus, onConfirm, onCancel }: {
  title: ReactNode; children?: ReactNode; confirmLabel: ReactNode; cancelLabel?: ReactNode;
  icon?: DialogIcon; tone?: Exclude<DialogTone, "default">; busy?: boolean; disabled?: boolean; wide?: boolean;
  /** For an answer safe to give by pressing Enter straight away. */
  focusConfirm?: boolean; returnFocus?: RefObject<HTMLElement | null>;
  onConfirm: () => void; onCancel: () => void;
}) {
  const cancel = () => { if (!busy) onCancel(); };
  return <AppDialog title={title} wide={wide} returnFocus={returnFocus} onCancel={cancel} actions={<>
    <DialogButton icon="cancel" disabled={busy} onClick={cancel}>{cancelLabel ?? t("common.cancel")}</DialogButton>
    <DialogButton tone={tone} icon={icon} disabled={busy || disabled} onClick={onConfirm} {...(focusConfirm ? { "data-dialog-default": "" } : {})}>{confirmLabel}</DialogButton>
  </>}>{children}</AppDialog>;
}
