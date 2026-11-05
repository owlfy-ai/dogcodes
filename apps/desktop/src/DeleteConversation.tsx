import { useRef, useState } from "react";
import type { TaskSummary } from "@dogcodes/protocol";
import { ConfirmDialog } from "./AppDialog";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";
import { taskDisplayTitle } from "./displayNames";

export function DeleteConversation({ task, blocked, onDelete, text = false }: {
  task: TaskSummary;
  blocked: boolean;
  onDelete: (task: TaskSummary) => Promise<void>;
  text?: boolean;
}) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const guard = useRef(false), trigger = useRef<HTMLButtonElement>(null);
  async function remove() {
    if (guard.current || blocked) return;
    guard.current = true; setBusy(true); setError("");
    try { await onDelete(task); setConfirming(false); }
    catch (cause) { setError(userFacingError(cause, t("deleteTask.failed"))); }
    finally { guard.current = false; setBusy(false); }
  }
  return <>
    <button ref={trigger} type="button" data-tooltip={blocked ? t("deleteTask.blockedTooltip") : t("nav.deleteTask")}
      aria-label={t("nav.deleteTask")} disabled={blocked} onClick={() => { setError(""); setConfirming(true); }}>
      {text ? t("common.delete") : <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 5h14M7 5V3h6v2M5 5l1 12h8l1-12M8 8v6m4-6v6" /></svg>}
    </button>
    {confirming && <ConfirmDialog returnFocus={trigger} title={t("deleteTask.title")} tone="danger" icon="trash" busy={busy} disabled={blocked}
      confirmLabel={t(busy ? "deleteTask.deleting" : "deleteTask.confirm")} onConfirm={() => void remove()} onCancel={() => setConfirming(false)}>
      <p>{t("deleteTask.warning", { title: taskDisplayTitle(task.title) })}</p>
      <p>{t("deleteTask.keeps")}</p>
      {blocked && <p role="alert">{t("deleteTask.blocked")}</p>}
      {error && <p role="alert">{error}</p>}
    </ConfirmDialog>}
  </>;
}
