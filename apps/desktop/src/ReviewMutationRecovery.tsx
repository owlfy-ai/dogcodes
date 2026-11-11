import { useEffect, useState, useSyncExternalStore } from "react";
import { mutationArguments, partialMutationResult, reviewMutations, type PendingMutation } from "./reviewMutations";
import "./gitOperationRecovery.css";
import { subscribeReviewChanges } from "./reviewWatch";
import { translate as t } from "./i18n";
import { displayBackendText } from "./backendText";

function label(item: PendingMutation) {
  const args = mutationArguments(item);
  if (item.request.command === "core_turn_undo") return t("mutationRecovery.undo");
  if (item.request.command === "core_review_index") return t(args.action === "stage" ? "mutationRecovery.stage" : "mutationRecovery.unstage");
  if (item.request.command === "core_review_batch_files") return t(args.action === "discard" ? "mutationRecovery.batchDiscard" : "mutationRecovery.batchIgnore");
  return t(item.request.command === "core_review_ignore" ? "mutationRecovery.ignore" : "mutationRecovery.discard");
}
/**
 * File operations whose reply was lost (a dropped connection or a closed
 * window) are settled from the service's record without asking: one that
 * finished or never started is dropped, one still running is waited for. Only
 * a result the person has not seen — a failure, a partial run or an
 * interruption — is shown once, until they acknowledge it.
 */
export function ReviewMutationRecovery({ taskId, artifactId, visible = true, onChanged }: { taskId: string; artifactId?: string; visible?: boolean; onChanged: () => void }) {
  const raw = useSyncExternalStore(reviewMutations.subscribe, () => reviewMutations.snapshot(taskId), () => "[]");
  const parsed = JSON.parse(raw) as PendingMutation[] | { error: string };
  const items = Array.isArray(parsed) ? parsed.filter(item => !artifactId || (item.request.command === "core_turn_undo" && mutationArguments(item).artifactId === artifactId)) : [];
  const [busy, setBusy] = useState(false), [error, setError] = useState("");
  const ids = items.map(item => item.request.operationId).join(",");
  useEffect(() => {
    if (!visible || !ids) return;
    let active = true;
    const refresh = () => { void Promise.all(items.map(item => reviewMutations.check(item))).then(() => { if (active) setError(""); }, () => { /* checked again on the next change */ }); };
    refresh();
    const stop = subscribeReviewChanges(taskId, refresh);
    return () => { active = false; stop(); };
    // Query on opening, identity changes and service invalidations. Never auto-replay.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [taskId, artifactId, visible, ids]);
  const settled = items.filter(item => !item.sending && ["succeeded", "failed", "unknown"].includes(item.receipt?.outcome.status ?? ""));
  if (!settled.length) return null;
  const acknowledge = async (item: PendingMutation) => {
    setBusy(true); setError("");
    try { await reviewMutations.acknowledge(item); onChanged(); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  };
  return <section className="git-operation-recovery" aria-label={t("mutationRecovery.label")}>
    {settled.map(item => {
      const state = item.receipt!.outcome, args = mutationArguments(item);
      const paths = Array.isArray(args.paths) ? args.paths.join("、") : args.path;
      const partial = state.status === "succeeded" ? partialMutationResult(state.result) : null;
      return <div key={item.request.operationId}>
        <p><strong>{label(item)}</strong>{typeof paths === "string" && ` · ${paths}`}</p>
        <p role="status">{partial ? t("mutationRecovery.partial", { count: partial.completed.length, error: displayBackendText(partial.error) })
          : state.status === "failed" ? t("mutationRecovery.failed", { message: displayBackendText(state.message) }) : t("mutationRecovery.uncertain")}</p>
        {partial && partial.completed.length > 0 && <details><summary>{t("mutationRecovery.completedFiles")}</summary><ul>{partial.completed.map(path => <li key={path}>{path}</li>)}</ul></details>}
        <div className="git-operation-recovery-actions">
          <button type="button" disabled={busy} onClick={() => void acknowledge(item)}>{t("mutationRecovery.acknowledge")}</button>
        </div>
      </div>;
    })}
    {error && <p role="alert">{displayBackendText(error)}</p>}
  </section>;
}
