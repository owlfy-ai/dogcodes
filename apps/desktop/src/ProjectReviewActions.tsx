import { ReviewStartRecovery, usePendingReview } from "./ReviewStartRecovery";
import { reviewStartOperations } from "./reviewStartOperations";
import { useEnabledAgents } from "./agentPreferences";
import { useContext, useEffect, useId, useRef, useState } from "react";
import { ulid } from "ulid";
import type { CoreClient, ExecutorRef, ProjectReviewRequest, ReviewKind, ReviewFinding, TaskId, TurnReviewRun } from "@dogcodes/protocol";
import { AgentQuotaContext } from "./AgentQuotaRing";
import { TauriCoreClient } from "./tauriClient";
import { reviewAgentExecutors } from "./agentOptions";
import { ProjectReviewHistory, ProjectReviewPage, ReviewHistoryTitle, newestReviews, reviewActive as active } from "./ProjectReviewHistory";
import { ReviewActionButton, ReviewAgentPicker, reviewKinds, reviewLabels as labels } from "./ReviewActions";
import { ReviewResult } from "./ReviewResult";
import { ReviewFailure } from "./reviewFailure";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import type { ReviewSnapshot } from "./reviewClient";
import "./projectReview.css";
import { translate as t } from "./i18n";

export const projectAnalysisClient = new TauriCoreClient();
const key = (executor: ExecutorRef) => `${executor.providerId}:${executor.installationId ?? ""}`;
const COLLAPSED_KEY = "dogcodes.review.collapsed";

export function ProjectReviewActions({ taskId, snapshot, disabled, onReviewFinding, onExpandedChange, onInitialLoad, visible = true, client = projectAnalysisClient }: {
  taskId: string; snapshot: ReviewSnapshot | null; disabled: boolean; onReviewFinding?: (finding: ReviewFinding) => void; onExpandedChange?: (expanded: boolean) => void; onInitialLoad?: () => void; visible?: boolean; client?: CoreClient;
}) {
  const [discoveredAgents, setExecutors] = useState<ExecutorRef[]>([]);
  const executors = useEnabledAgents(discoveredAgents);
  const [selected, setSelected] = useState(() => { try { return localStorage.getItem("dogcodes.review.executor") ?? ""; } catch { return ""; } });
  const quotas = useContext(AgentQuotaContext);
  const [menu, setMenu] = useState(false);
  const [runs, setRuns] = useState<TurnReviewRun[]>([]);
  const [error, setError] = useState("");
  const [readError, setReadError] = useState("");
  const [agentError, setAgentError] = useState("");
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [agentsLoaded, setAgentsLoaded] = useState(false);
  const [loadedTask, setLoadedTask] = useState<string | null>(null);
  const orderedRuns = newestReviews(runs);
  const selectedRun = orderedRuns.find(run => run.request.runId === selectedRunId) ?? orderedRuns[0];
  const [collapsed, setCollapsedState] = useState(() => { try { return localStorage.getItem(COLLAPSED_KEY) === "1"; } catch { return false; } });
  const setCollapsed = (value: boolean) => {
    setCollapsedState(value);
    try { localStorage.setItem(COLLAPSED_KEY, value ? "1" : "0"); } catch { /* Optional preference. */ }
  };
  const bodyId = useId();
  const expanded = runs.length > 0 && !collapsed;
  useEffect(() => { onExpandedChange?.(expanded); }, [expanded, onExpandedChange]);
  useEffect(() => {
    if (agentsLoaded && loadedTask === taskId) onInitialLoad?.();
  }, [agentsLoaded, loadedTask, taskId, onInitialLoad]);
  const mounted = useRef(true), guard = useRef(false);
  const pending = usePendingReview(taskId as TaskId, "project");
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => {
    let valid = true;
    void Promise.all([client.bootstrap(), client.reviewExecutors?.() ?? []]).then(([bootstrap, connected]) => {
      if (valid) { setExecutors(reviewAgentExecutors(bootstrap.installations, connected)); setAgentError(""); }
    }).catch(() => { if (valid) setAgentError(t("projectReview.agentsFailed")); })
      .finally(() => { if (valid) setAgentsLoaded(true); });
    return () => { valid = false; };
  }, [client, menu]);
  useEffect(() => {
    if (!visible) return;
    let valid = true; let timer: ReturnType<typeof setTimeout>;
    const load = async () => {
      let interval = 10000;
      try {
        const all = await client.turnReviews?.(taskId as TaskId) ?? [];
        if (!valid) return;
        const projectRuns = all.filter(run => run.request.turnId === null && run.request.sourceSnapshot);
        setRuns(projectRuns); setReadError("");
        setSelectedRunId(previous => projectRuns.some(run => run.request.runId === previous) ? previous : newestReviews(projectRuns)[0]?.request.runId ?? null);
        if (projectRuns.some(active)) interval = 1500;
      } catch { if (valid) setReadError(t("projectReview.statusFailed")); }
      finally { if (valid) setLoadedTask(taskId); }
      // Also discover reviews started in a detached window or another panel.
      if (valid) timer = setTimeout(load, interval);
    };
    void load();
    return () => { valid = false; clearTimeout(timer); };
  }, [client, taskId, revision, visible]);
  const executor = executors.find(e => key(e) === selected) ?? executors.find(e => e.providerId === selected.split(":")[0]) ?? executors[0];
  const count = snapshot?.roots.reduce((total, root) => total + root.files.length, 0) ?? 0;
  const reason = disabled ? t("projectReview.waitDiff") : !snapshot || !count ? t("projectReview.noChanges") : snapshot.roots.some(root => root.error) ? t("projectReview.partialRead") : !executor ? t("projectReview.noAgent") : !client.startProjectReview ? t("turnActions.desktopOnly") : "";
  async function start(kind: ReviewKind, retry?: TurnReviewRun) {
    if (!executor || !client.startProjectReview || guard.current || pending || (!retry && reason)) return;
    const sourceSnapshot = retry?.workspaceReview ? retry.request.sourceSnapshot : snapshot?.id;
    if (!sourceSnapshot) return;
    const retryOf = retry?.workspaceReview ? retry.request.runId : null;
    const request: ProjectReviewRequest = { taskId: taskId as TaskId, turnId: null, sourceSnapshot, kind, executor, runId: `run_${ulid()}`, retryOf };
    guard.current = true; setBusy(true); setError(""); setCollapsed(false);
    try {
      const run = await reviewStartOperations.send(client, "project", request);
      if (mounted.current) { setRuns(oldRuns => [...oldRuns.filter(r => r.request.runId !== run.request.runId), run]); setSelectedRunId(run.request.runId); setRevision(n => n + 1); }
    } catch (cause) { if (mounted.current) setError(userFacingError(cause, t("turnActions.startFailed"))); }
    finally { guard.current = false; if (mounted.current) setBusy(false); }
  }
  async function cancel(run: TurnReviewRun) {
    try { await client.cancelTurnReview?.(taskId as TaskId, run.request.runId); if (mounted.current) setRevision(n => n + 1); }
    catch (cause) { if (mounted.current) setError(userFacingError(cause, t("turnActions.cancelFailed"))); }
  }
  const title = <><svg className="project-review-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg><span className="project-review-title">{t("projectReview.title")}</span></>;
  // With nothing to review and no past run, the bar stays a quiet one-line hint.
  const idle = !selectedRun && (!snapshot || !count);
  return <section className="project-review" aria-label={t("projectReview.label")} data-expanded={expanded || undefined} data-idle={idle || undefined}>
    <header className="project-review-header">
      {selectedRun
        ? <button type="button" className="project-review-toggle" aria-expanded={expanded} aria-controls={bodyId} data-tooltip={t(expanded ? "projectReview.collapse" : "projectReview.expand")} onClick={() => setCollapsed(expanded)}>
          {title}{!expanded && <span className="project-review-collapsed-status"><ReviewHistoryTitle run={selectedRun}/></span>}
        </button>
        : <span className="project-review-toggle" aria-disabled="true">{title}<small className="project-review-hint" data-tooltip={reason || undefined}>{reason || t("projectReview.startHint", { count })}</small></span>}
      <div className="project-review-toolbar">
        {reviewKinds.map(kind => <ReviewActionButton key={kind} kind={kind} disabled={busy || !!pending || !!reason || runs.some(run => active(run) && run.request.sourceSnapshot === snapshot?.id && run.request.kind === kind)} tooltip={reason || t("projectReview.kindTooltip", { kind: labels[kind], count })} onClick={() => void start(kind)}/>)}
        <ReviewAgentPicker executors={executors} executor={executor} quotas={quotas} open={menu} onOpenChange={setMenu} onSelect={setSelected} label={t("projectReview.selectAgent")}/>
      </div>
    </header>
    {(error || readError || agentError) && <div className="project-review-messages"><ReviewFailure error={error}/>{readError && <InlineError role="status">{readError}</InlineError>}{agentError && <InlineError role="status">{agentError}</InlineError>}</div>}
    <ReviewStartRecovery client={client} taskId={taskId as TaskId} scope="project" busy={busy} onRecovered={run => { setRuns(old => [...old.filter(value => value.request.runId !== run.request.runId), run]); setSelectedRunId(run.request.runId); setCollapsed(false); setRevision(value => value + 1); setError(""); }}/>
    {selectedRun && <div id={bodyId} className="project-review-body" hidden={!expanded}>
      <div className="project-review-history-row">
        <ProjectReviewHistory runs={orderedRuns} selected={selectedRun} onSelect={setSelectedRunId}/>
        {active(selectedRun) && <button type="button" className="project-review-cancel" disabled={selectedRun.status === "cancelling"} onClick={() => void cancel(selectedRun)}>{t("turnActions.cancelReview")}</button>}
      </div>
      {orderedRuns.map(run => <ProjectReviewPage key={run.request.runId} runId={run.request.runId} selected={run.request.runId === selectedRun.request.runId}>
        <ReviewResult client={client} run={run} executor={executor} onReviewFinding={onReviewFinding}
          onRetry={run.workspaceReview && ["failed", "interrupted"].includes(run.status) && reviewKinds.some(kind => kind === run.request.kind)
            ? () => void start(run.request.kind as ReviewKind, run) : undefined}
          retryDisabled={busy || !!pending || !executor || !client.startProjectReview || runs.some(other => active(other) && other.request.kind === run.request.kind && other.request.sourceSnapshot === run.request.sourceSnapshot)}
          retryTooltip={t(!executor ? "projectReview.noReviewAgent" : "projectReview.retryTooltip")}/>
      </ProjectReviewPage>)}
    </div>}
  </section>;
}
