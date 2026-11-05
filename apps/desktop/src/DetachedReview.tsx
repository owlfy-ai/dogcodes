import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { BootstrapSnapshot, QuotaSnapshotView, TaskId } from "@dogcodes/protocol";
import { AgentQuotaContext } from "./AgentQuotaRing";
import { TargetOfflinePanel } from "./ExecutionTargets";
import { GitCredentialPrompt } from "./GitCredentialPrompt";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { projectAnalysisClient } from "./ProjectReviewActions";
import { createQuotaRefresh } from "./quotaRefresh";
import { ReviewPanel, type ReviewView } from "./ReviewPanel";
import { translate as t } from "./i18n";

export function DetachedReview({ taskId }: { taskId: string }) {
  const [running, setRunning] = useState(true);
  const [error, setError] = useState("");
  const [quotas, setQuotas] = useState<QuotaSnapshotView[]>([]);
  const [initialView] = useState<ReviewView | undefined>(() => (window as Window & { __REVIEW_VIEW__?: ReviewView }).__REVIEW_VIEW__);
  // This window has no App provider. Load independently of menu interactions,
  // publishing each provider as it completes instead of waiting for all of them.
  useEffect(() => {
    setQuotas([]);
    const reader = createQuotaRefresh(projectAnalysisClient, taskId as TaskId, view => {
      setQuotas(previous => [...previous.filter(old => old.installationId !== view.installationId), view]);
    });
    const load = () => { void reader.refresh().catch(() => {}); };
    load();
    const timer = setInterval(load, 30_000);
    return () => { clearInterval(timer); reader.dispose(); };
  }, [taskId]);
  useEffect(() => {
    let active = true; let pending = false;
    const check = async () => {
      if (pending) return; pending = true;
      try { const value = await invoke<boolean>("core_review_window_running", { taskId }); if (active) { setRunning(value); setError(""); } }
      catch (e) { if (active) { setRunning(true); setError(userFacingError(e, t("detachedReview.statusFailed"))); } }
      finally { pending = false; }
    };
    void check(); const timer = setInterval(() => void check(), 2000);
    const theme = () => {
      const preferred = localStorage.getItem("dogcodes.ui.theme");
      document.documentElement.dataset.theme = preferred === "light" || preferred === "dark" ? preferred : matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
    };
    theme(); const media = matchMedia("(prefers-color-scheme: light)");
    window.addEventListener("storage", theme); media.addEventListener("change", theme);
    return () => { active = false; clearInterval(timer); window.removeEventListener("storage", theme); media.removeEventListener("change", theme); };
  }, [taskId]);
  // Undefined until known, so a server's project never starts reading while offline.
  const [target, setTarget] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    let active = true;
    void invoke<BootstrapSnapshot>("core_bootstrap").then(snapshot => {
      const projectId = snapshot.tasks.find(task => task.taskId === taskId)?.projectId;
      if (active) setTarget(snapshot.projects.find(project => project.projectId === projectId)?.targetId ?? null);
    }).catch(() => { if (active) setTarget(null); });
    return () => { active = false; };
  }, [taskId]);
  const close = () => { void invoke("core_review_window_close", { taskId }).catch(e => setError(userFacingError(e, t("detachedReview.closeFailed")))); };
  const panel = <ReviewPanel workspaceNavigation taskId={taskId} running={running} initialView={initialView} onClose={close} standalone />;
  return <main className="detached-review-shell">
    {error && <InlineError>{error}</InlineError>}
    <GitCredentialPrompt taskId={taskId} />
    <AgentQuotaContext.Provider value={quotas}>
      {target === undefined ? null : target
        ? <TargetOfflinePanel targetId={target} visible standalone onClose={close}>{panel}</TargetOfflinePanel>
        : panel}
    </AgentQuotaContext.Provider>
  </main>;
}
