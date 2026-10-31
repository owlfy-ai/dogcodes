import { focusDialogTitle } from "./dialogFocus";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { subscribeReviewChanges } from "./reviewWatch";
import { mutationArguments, reviewMutations, type PendingMutation } from "./reviewMutations";
import { ReviewMutationRecovery } from "./ReviewMutationRecovery";
import type { ArtifactMetadata, TaskId } from "@dogcodes/protocol";
import { translate as t } from "./i18n";

export function TurnUndoAction({ taskId, metadata, count }: { taskId: TaskId; metadata: ArtifactMetadata; count: number }) {
  const [undone, setUndone] = useState(false), [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  const [statusError, setStatusError] = useState("");
  const raw = useSyncExternalStore(reviewMutations.subscribe, () => reviewMutations.snapshot(taskId), () => "[]");
  const saved = JSON.parse(raw) as PendingMutation[] | { error: string };
  const pending = !Array.isArray(saved) || saved.some(item => item.request.command === "core_turn_undo" && mutationArguments(item).artifactId === metadata.artifactId);
  const statusRevision = useRef(0), refreshStatus = useRef<() => void>(() => {});
  const dialog = useRef<HTMLDialogElement>(null), trigger = useRef<HTMLButtonElement>(null), guard = useRef(false);
  useEffect(() => {
    let active = true;
    setUndone(false);
    const refresh = async () => {
      const revision = ++statusRevision.current;
      try {
        const value = await invoke<boolean>("core_turn_undo_status", { taskId, artifactId: metadata.artifactId });
        if (active && revision === statusRevision.current) { setUndone(Boolean(value)); setStatusError(""); }
      } catch { if (active && revision === statusRevision.current) setStatusError(t("undo.statusFailed")); }
    };
    refreshStatus.current = () => { void refresh(); };
    void refresh();
    const stop = subscribeReviewChanges(taskId, () => { void refresh(); });
    return () => { active = false; statusRevision.current++; stop(); };
  }, [taskId, metadata.artifactId]);
  useEffect(() => {
    if (!confirming) return;
    dialog.current?.showModal();
    focusDialogTitle(dialog.current);
    return () => { dialog.current?.close(); trigger.current?.focus(); };
  }, [confirming]);
  async function undo() {
    if (guard.current || pending || undone) return;
    guard.current = true; setBusy(true); setError("");
    try {
      await reviewMutations.run("core_turn_undo", { request: { taskId, artifactId: metadata.artifactId, expectedPayloadSha256: metadata.payloadSha256, confirmed: true } }, taskId, metadata.artifactId);
      statusRevision.current++; setUndone(true); setConfirming(false);
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { guard.current = false; setBusy(false); }
  }
  return <>
    <button ref={trigger} className="turn-summary-undo" type="button" disabled={(!pending && undone) || busy}
      aria-label={t(pending ? "undo.verifyLabel" : undone ? "undo.undoneLabel" : "undo.label")}
      onClick={event => { event.preventDefault(); event.stopPropagation(); setError(""); setConfirming(true); }}>
      {!undone && <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><path d="m6 3-4 4 4 4M2 7h7a4 4 0 0 1 0 8" /></svg>}
      {t(pending ? "undo.verify" : undone ? "undo.undone" : "undo.action")}
    </button>
    {confirming && createPortal(<dialog ref={dialog} className="turn-undo-dialog" aria-labelledby="turn-undo-title"
      onClick={event => event.stopPropagation()} onCancel={event => { event.preventDefault(); if (!guard.current) setConfirming(false); }}>
      <h2 id="turn-undo-title" tabIndex={-1} data-dialog-title>{t("undo.title")}</h2>
      <p>{t("undo.description", { count })}</p>
      <p className="turn-undo-hint">{t("undo.hint")}</p>
      {statusError && <p role="status">{statusError}</p>}
      {error && <p role="alert">{error}</p>}
      <ReviewMutationRecovery taskId={taskId} artifactId={metadata.artifactId} onChanged={() => { setError(""); refreshStatus.current(); }} />
      <footer><button type="button" disabled={busy} onClick={() => setConfirming(false)}>{t("common.cancel")}</button>
        {!pending && !undone && <button className="turn-undo-confirm" type="button" disabled={busy} onClick={() => void undo()}>{t(busy ? "undo.undoing" : "undo.confirm")}</button>}</footer>
    </dialog>, document.body)}
  </>;
}
