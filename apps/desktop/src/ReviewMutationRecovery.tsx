import { useEffect, useState, useSyncExternalStore } from "react";
import { mutationArguments, partialMutationResult, reviewMutations, type PendingMutation } from "./reviewMutations";
import "./gitOperationRecovery.css";
import { subscribeReviewChanges } from "./reviewWatch";
import { translate as t } from "./i18n";

function label(item: PendingMutation) {
  const args = mutationArguments(item);
  if (item.request.command === "core_turn_undo") return t("mutationRecovery.undo");
  if (item.request.command === "core_review_index") return t(args.action === "stage" ? "mutationRecovery.stage" : "mutationRecovery.unstage");
  if (item.request.command === "core_review_batch_files") return t(args.action === "discard" ? "mutationRecovery.batchDiscard" : "mutationRecovery.batchIgnore");
  return t(item.request.command === "core_review_ignore" ? "mutationRecovery.ignore" : "mutationRecovery.discard");
}
export function ReviewMutationRecovery({ taskId, artifactId, visible = true, onChanged }: { taskId: string; artifactId?: string; visible?: boolean; onChanged: () => void }) {
  const raw = useSyncExternalStore(reviewMutations.subscribe, () => reviewMutations.snapshot(taskId), () => "[]");
  const parsed = JSON.parse(raw) as PendingMutation[] | { error: string };
  const items = Array.isArray(parsed) ? parsed.filter(item => !artifactId || (item.request.command === "core_turn_undo" && mutationArguments(item).artifactId === artifactId)) : [];
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  const ids = items.map(item => item.request.operationId).join(",");
  useEffect(() => {
    if (!visible || !ids) return;
    let active = true;
    const refresh = () => { void Promise.all(items.map(item => reviewMutations.check(item))).catch(cause => { if (active) setError(String(cause)); }); };
    refresh();
    const stop = subscribeReviewChanges(taskId, refresh);
    return () => { active = false; stop(); };
    // Query on opening, identity changes and service invalidations. Never auto-replay.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [taskId, artifactId, visible, ids]);
  if (!Array.isArray(parsed)) return <p role="alert">{parsed.error}</p>;
  if (!items.length) return null;
  const run = async (action: () => unknown) => {
    setBusy(true); setError("");
    try { await action(); onChanged(); } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  };
  return <section className="git-operation-recovery" aria-label={t("mutationRecovery.label")}>
    {items.map(item => {
      const state = item.receipt?.outcome, args = mutationArguments(item);
      const paths = Array.isArray(args.paths) ? args.paths.join("、") : args.path;
      const partial = state?.status === "succeeded" ? partialMutationResult(state.result) : null;
      const terminal = state?.status === "succeeded" || state?.status === "failed" || state?.status === "unknown";
      return <div key={item.request.operationId}>
        <p><strong>{label(item)}</strong>{typeof paths === "string" && ` · ${paths}`}</p>
        <p role="status">{partial ? t("mutationRecovery.partial", { count: partial.completed.length, error: partial.error }) : state?.status === "succeeded" ? t("mutationRecovery.succeeded") : state?.status === "failed" ? t("mutationRecovery.failed", { message: state.message }) : state?.status === "running" ? t("mutationRecovery.running") : item.receipt === null ? t("mutationRecovery.notFound") : t("mutationRecovery.uncertain")}</p>
        {partial && partial.completed.length > 0 && <details><summary>{t("mutationRecovery.completedFiles")}</summary><ul>{partial.completed.map(path => <li key={path}>{path}</li>)}</ul></details>}
        <div className="git-operation-recovery-actions">
          <button type="button" disabled={busy || item.sending} onClick={() => void run(() => reviewMutations.check(item))}>{t("mutationRecovery.verify")}</button>
          {item.receipt === null && <button type="button" disabled={busy || item.sending} onClick={() => void run(() => reviewMutations.resend(item))}>{t("mutationRecovery.resend")}</button>}
          {terminal && <button type="button" disabled={busy || item.sending} onClick={() => void run(() => reviewMutations.acknowledge(item))}>{t(state?.status === "succeeded" && !partial ? "turnCommit.done" : "mutationRecovery.acknowledge")}</button>}
        </div>
      </div>;
    })}
    {error && <p role="alert">{error}</p>}
  </section>;
}
