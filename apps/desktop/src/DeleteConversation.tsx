import { useRef, useState, type RefObject } from "react";
import type { TaskSummary } from "@dogcodes/protocol";
import { ConfirmDialog } from "./AppDialog";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";
import { taskDisplayTitle } from "./displayNames";

export function DeleteConversationDialog({ task, blocked, onDelete, onClose, returnFocus }: {
  task: TaskSummary;
  blocked: boolean;
  onDelete: (task: TaskSummary) => Promise<void>;
  onClose: () => void;
  returnFocus?: RefObject<HTMLElement | null>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const guard = useRef(false);
  async function remove() {
    if (guard.current || blocked) return;
    guard.current = true; setBusy(true); setError("");
    try { await onDelete(task); onClose(); }
    catch (cause) { setError(userFacingError(cause, t("deleteTask.failed"))); }
    finally { guard.current = false; setBusy(false); }
  }
  return <ConfirmDialog returnFocus={returnFocus} title={t("deleteTask.title")} tone="danger" icon="trash" busy={busy} disabled={blocked}
    confirmLabel={t(busy ? "deleteTask.deleting" : "deleteTask.confirm")} onConfirm={() => void remove()} onCancel={onClose}>
    <p>{t("deleteTask.warning", { title: taskDisplayTitle(task.title) })}</p>
    <p>{t("deleteTask.keeps")}</p>
    {blocked && <p role="alert">{t("deleteTask.blocked")}</p>}
    {error && <p role="alert">{error}</p>}
  </ConfirmDialog>;
}

export function DeleteConversation({ task, blocked, onDelete, text = false }: {
  task: TaskSummary;
  blocked: boolean;
  onDelete: (task: TaskSummary) => Promise<void>;
  text?: boolean;
}) {
  const [confirming, setConfirming] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  return <>
    <button ref={trigger} type="button" data-tooltip={blocked ? t("deleteTask.blockedTooltip") : t("nav.deleteTask")}
      aria-label={t("nav.deleteTask")} disabled={blocked} onClick={() => setConfirming(true)}>
      {text ? t("common.delete") : <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 5h14M7 5V3h6v2M5 5l1 12h8l1-12M8 8v6m4-6v6" /></svg>}
    </button>
    {confirming && <DeleteConversationDialog task={task} blocked={blocked} onDelete={onDelete} returnFocus={trigger} onClose={() => setConfirming(false)} />}
  </>;
}
