import { ReviewMutationRecovery } from "./ReviewMutationRecovery";
import { keepReviewLease } from "./reviewLease";
import { ReviewWorkspaceBrowser, type ReviewWorkspaceApi } from "./ReviewWorkspaceBrowser";
import type { ReactNode } from "react";
import { ReviewGitMenu, ReviewGitRemoteStatus, type GitRemoteNotice } from "./ReviewGitMenu";
import { useReviewFileSelection } from "./useReviewFileSelection";
import { ReviewSelectionActions } from "./ReviewSelectionActions";
import { ProjectReviewActions } from "./ProjectReviewActions";
import { ReviewDiffSplit } from "./ReviewDiffSplit";
import { ReviewFilesSplit } from "./ReviewFilesSplit";
import type { CoreClient, TaskSummary } from "@dogcodes/protocol";
import { WorkspaceFiles } from "./WorkspaceFiles";
import { LoadingStatus } from "./LoadingStatus";
import { WorkspaceTabs, type WorkspaceTab } from "./WorkspaceTabs";
import { ReviewGitActions } from "./ReviewGitActions";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { ReviewBinaryPlaceholder } from "./ReviewBinaryPlaceholder";
import { reviewTarget, reviewFindingTarget } from "./reviewTarget";
import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ReviewText } from "./ReviewText";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { ReviewFileLoading } from "./ReviewFileLoading";
import { ReviewPanelLoading } from "./ReviewPanelLoading";
import { ReviewFileTree, directoryName } from "./ReviewFileTree";
import { ReviewFileMenu, type FileMenuTarget } from "./ReviewFileMenu";
import { WorkspaceFilePreview } from "./WorkspaceFilePreview";
import { reviewFingerprint } from "./reviewFingerprint";
import { useLoadingVisibility } from "./useLoadingVisibility";
import { reviewClient, type ReviewSnapshot, type ReviewRoot, type ReviewMode, type ReviewFile, type ReviewCommit, type ReviewBranch, type RootChanges, type FilePreview } from "./reviewClient";
import { translate as t } from "./i18n";

const ProjectRunPanel = lazy(() => import("./ProjectRunPanel").then(module => ({ default: module.ProjectRunPanel })));

export interface ReviewView {
  tab?: WorkspaceTab;
  directoryId?: string;
  rootId: string; mode: ReviewMode; reference: string; parent: number;
  split: boolean; filter: string; fileView?: "list" | "tree";
  selection: { rootId: string; path: string } | null; roots: ReviewRoot[];
}

/** Retain visited readers until their conversations are removed. Unopened docks do no work. */
export function ConversationReviewDocks({ tasks, selectedTaskId, open, fileRequest, ...props }: Omit<React.ComponentProps<typeof ReviewPanel>, "taskId" | "running" | "visible" | "requestedPath" | "fileRequestId"> & {
  tasks: Pick<TaskSummary, "taskId" | "activePrimaryRun">[];
  selectedTaskId: string | null;
  open: boolean;
  fileRequest?: { taskId: string; path: string; id: number } | null;
}) {
  const previousDrawer = useRef({ open: false, taskId: null as string | null });
  const [enteringTaskId, setEnteringTaskId] = useState<string | null>(null);
  useLayoutEffect(() => {
    // Only opening the shared drawer animates. Showing another cached reader
    // (or mounting a new one) while it is already open must not slide it in.
    const previous = previousDrawer.current;
    if (previous.open === open && previous.taskId === selectedTaskId) return;
    setEnteringTaskId(open && !previous.open ? selectedTaskId : null);
    previousDrawer.current = { open, taskId: selectedTaskId };
  }, [open, selectedTaskId]);
  return <>{tasks.map(task => <ReviewDock {...props} key={task.taskId} taskId={task.taskId}
    animateEntrance={open && task.taskId === selectedTaskId && task.taskId === enteringTaskId}
    open={open && task.taskId === selectedTaskId} running={Boolean(task.activePrimaryRun)}
    requestedPath={fileRequest?.taskId === task.taskId ? fileRequest.path : undefined}
    fileRequestId={fileRequest?.taskId === task.taskId ? fileRequest.id : undefined} />)}</>;
}

// Keep this task's reader mounted after its first opening.
export function ReviewDock({ open, ...props }: React.ComponentProps<typeof ReviewPanel> & { open: boolean }) {
  const [opened, setOpened] = useState(open);
  useEffect(() => { if (open) setOpened(true); }, [open]);
  return open || opened ? <ReviewPanel {...props} visible={open} /> : null;
}

export interface ReviewPanelProps {
  /** Opens the project editor, so empty states can offer adding a directory. */
  onEditProject?: () => void;
  taskId: string; running: boolean; onClose: () => void; api?: typeof reviewClient; analysisClient?: CoreClient; visible?: boolean; animateEntrance?: boolean; onDetach?: (view: ReviewView) => void; standalone?: boolean; initialView?: ReviewView; requestedPath?: string; fileRequestId?: number;
  workspaceNavigation?: boolean; workspaceApi?: ReviewWorkspaceApi; onOpenConversation?: (taskId: string) => void;
}
export function ReviewPanel(props: ReviewPanelProps) {
  if (!props.workspaceNavigation) return <ReviewReader {...props}/>;
  return <ReviewWorkspaceBrowser key={props.taskId} taskId={props.taskId} running={props.running} visible={props.visible}
    fileRequestId={props.fileRequestId} requestedPath={props.requestedPath} api={props.workspaceApi} onOpenConversation={props.onOpenConversation}>
    {target => <ReviewReader {...props} taskId={target.taskId} running={target.running} visible={target.visible}
      animateEntrance={target.current && props.animateEntrance} workspaceHeader={target.header} onOperationBusyChange={target.onBusy} onWorkspaceChanged={target.onChanged}
      onDetach={target.current ? props.onDetach : undefined} initialView={target.current ? props.initialView : undefined}
      requestedPath={target.current ? props.requestedPath : undefined} fileRequestId={target.current ? props.fileRequestId : undefined}/>}
  </ReviewWorkspaceBrowser>;
}
function ReviewReader({ taskId, running, onClose, onEditProject, api = reviewClient, analysisClient, visible = true, animateEntrance = false, onDetach, standalone = false, initialView, requestedPath, fileRequestId, workspaceHeader, onOperationBusyChange, onWorkspaceChanged }: ReviewPanelProps & {
  workspaceHeader?: ReactNode; onOperationBusyChange?: (busy: boolean) => void; onWorkspaceChanged?: () => void;
}) {
  const diffBody = useRef<HTMLDivElement>(null);
  const [expandedReviewTask, setExpandedReviewTask] = useState<string | null>(null);
  const onReviewExpanded = useCallback((expanded: boolean) => setExpandedReviewTask(expanded ? taskId : null), [taskId]);
  const [analysisReadyTask, setAnalysisReadyTask] = useState<string | null>(null);
  const onAnalysisReady = useCallback(() => setAnalysisReadyTask(taskId), [taskId]);
  const [revealDiff, setRevealDiff] = useState(requestedPath ? 1 : 0);
  const [runOpened, setRunOpened] = useState(initialView?.tab === "run");
  const [filesOpened, setFilesOpened] = useState(initialView?.tab === "files");
  const [tab, setTab] = useState<WorkspaceTab>(requestedPath ? "changes" : initialView?.tab === "files" || initialView?.tab === "run" ? initialView.tab : "changes");
  const [snapshot, setSnapshot] = useState<ReviewSnapshot | null>(null);
  const pendingPath = useRef(requestedPath);
  const [targetError, setTargetError] = useState("");
  const pendingSelection = useRef<{ rootId: string; path: string } | null>(initialView?.selection ?? null);
  const [roots, setRoots] = useState<ReviewRoot[]>(initialView?.roots ?? []);
  const [directoryId, setDirectoryId] = useState(initialView?.directoryId ?? (initialView && initialView.mode !== "uncommitted" ? initialView.rootId : ""));
  const [rootId, setRootId] = useState(initialView?.rootId ?? "");
  const [mode, setMode] = useState<ReviewMode>(initialView?.mode ?? "uncommitted");
  const [reference, setReference] = useState(initialView?.reference ?? "");
  const [parent, setParent] = useState(initialView?.parent ?? 0);
  const [commits, setCommits] = useState<ReviewCommit[]>([]);
  const [branches, setBranches] = useState<ReviewBranch[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [historyBusy, setHistoryBusy] = useState(false);
  const [metadataError, setMetadataError] = useState("");
  const [revision, setRevision] = useState(0);
  const [syncedRevision, setSyncedRevision] = useState(0);
  const [pulledRevision, setPulledRevision] = useState(0);
  const [remoteNotice, setRemoteNotice] = useState<GitRemoteNotice | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [fileView, setFileView] = useState<"list" | "tree">(initialView?.fileView ?? "list");
  const filter = "";
  const [selection, setSelection] = useState<{ rootId: string; path: string } | null>(null);
  const [file, setFile] = useState<ReviewFile | null>(null);
  const [fileError, setFileError] = useState("");
  const [split, setSplit] = useState(initialView?.split ?? false);
  const [changed, setChanged] = useState(false);
  const [updateError, setUpdateError] = useState("");
  const [operationBusy, setOperationBusy] = useState(false);
  useEffect(() => { onOperationBusyChange?.(operationBusy); }, [operationBusy, onOperationBusyChange]);
  useEffect(() => { if (revision > 0) onWorkspaceChanged?.(); }, [revision, onWorkspaceChanged]);
  const [contextMenu, setContextMenu] = useState<FileMenuTarget | null>(null);
  const [previewTarget, setPreviewTarget] = useState<{ root: RootChanges; path: string; snapshotId: string; version: "working" | "head" } | null>(null);
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [previewError, setPreviewError] = useState("");
  const [treeRequest, setTreeRequest] = useState<{ path: string; id: number } | undefined>();
  const wasRunning = useRef(running);
  const metadataEpoch = useRef(0);
  const loadedIdentity = useRef("");
  const loadedSnapshot = useRef("");
  const queryScope = useRef("");
  const selectionRef = useRef(selection);
  selectionRef.current = selection;
  const fileIdentity = JSON.stringify([taskId, selection?.rootId, selection?.path, mode, reference, parent]);
  const scopeRoot = roots.find(r => r.id === directoryId) ?? (roots.length === 1 ? roots[0] : undefined);
  const root = scopeRoot ?? roots.find(r => r.id === rootId) ?? roots[0];
  const needsReference = mode === "committed" || mode === "branch";
  const queryRootId = directoryId || (needsReference ? scopeRoot?.id : undefined);
  const hasGit = scopeRoot ? scopeRoot.git : roots.some(r => r.git);
  const mixed = !scopeRoot && roots.some(r => r.git) && roots.some(r => !r.git);
  const modes: ReviewMode[] = hasGit ? scopeRoot ? ["uncommitted", "unstaged", "staged", "committed", "branch"] : ["uncommitted", "unstaged", "staged"] : [];
  const emptyDescriptions: Record<ReviewMode, string> = { uncommitted: t("reviewPanel.empty.uncommitted"), unstaged: t("reviewPanel.empty.unstaged"), staged: t("reviewPanel.empty.staged"), committed: t("reviewPanel.empty.committed"), branch: t("reviewPanel.empty.branch"), session: t("reviewPanel.empty.session") };
  const modeLabels = { uncommitted: t(mixed ? "reviewPanel.mode.current" : "reviewPanel.mode.uncommitted"), unstaged: t(mixed ? "reviewPanel.mode.unstagedGit" : "reviewPanel.mode.unstaged"), staged: t(mixed ? "reviewPanel.mode.stagedGit" : "reviewPanel.mode.staged"), committed: t("reviewPanel.mode.committed"), branch: t("reviewPanel.mode.branch"), session: t("reviewPanel.mode.session") };
  const selectionScope = JSON.stringify([taskId, queryRootId, mode, reference, parent]);
  const fileSelection = useReviewFileSelection(snapshot, selectionScope, busy || operationBusy || !visible || tab !== "changes");
  const closeContextMenu = useCallback(() => setContextMenu(null), []);
  const lastFileRequest = useRef({ path: requestedPath, id: fileRequestId });
  useEffect(() => {
    const previous = lastFileRequest.current;
    if (previous.path === requestedPath && previous.id === fileRequestId) return;
    lastFileRequest.current = { path: requestedPath, id: fileRequestId };
    if (!requestedPath) return;
    // Reuse the open panel; replacing its React key replays the drawer animation.
    pendingPath.current = requestedPath;
    setRevealDiff(value => value + 1);
    pendingSelection.current = null;
    selectionRef.current = null;
    loadedSnapshot.current = "";
    setDirectoryId(""); setTab("changes"); setMode("uncommitted"); setReference(""); setParent(0);
    setTargetError(""); setSelection(null); setFile(null); setPreviewTarget(null);
    setRevision(value => value + 1);
  }, [requestedPath, fileRequestId]);
  useEffect(() => {
    if (!snapshot) return;
    return keepReviewLease(api.lease, snapshot.taskId, snapshot.id, cause => {
      setChanged(true); setError(userFacingError(cause, t("reviewPanel.snapshotExpired")));
    });
  }, [api, snapshot]);
  useEffect(() => {
    if (wasRunning.current && !running && mode !== "committed") {
      // An open reader stays frozen until the user accepts the new version.
      // With no file open there is no reading position to disrupt.
      if (selectionRef.current || !visible) setChanged(true);
      else setRevision(n => n + 1);
    }
    wasRunning.current = running;
  }, [running, mode, visible]);
  useEffect(() => {
    const epoch = ++metadataEpoch.current;
    setCommits([]); setBranches([]); setHasMore(false); setMetadataError("");
    if (!queryRootId || !needsReference) { setHistoryBusy(false); return; }
    setHistoryBusy(true);
    const request = mode === "committed" ? api.history(taskId, queryRootId!) : api.branches(taskId, queryRootId!);
    void request.then(result => {
      if (metadataEpoch.current !== epoch) return;
      if (mode === "committed") { setCommits(result as ReviewCommit[]); setHasMore(result.length === 50); }
      else setBranches(result as ReviewBranch[]);
    }).catch(e => { if (metadataEpoch.current === epoch) setMetadataError(userFacingError(e, t(mode === "committed" ? "reviewPanel.historyFailed" : "reviewPanel.branchesFailed"))); })
      .finally(() => { if (metadataEpoch.current === epoch) setHistoryBusy(false); });
    return () => { metadataEpoch.current++; };
  }, [api, taskId, queryRootId, mode, needsReference]);
  const moreHistory = async () => {
    if (historyBusy) return;
    const epoch = metadataEpoch.current;
    setHistoryBusy(true); setMetadataError("");
    try {
      const result = await api.history(taskId, queryRootId!, commits.length);
      if (epoch === metadataEpoch.current) { setCommits(old => [...old, ...result]); setHasMore(result.length === 50); }
    } catch (e) { if (epoch === metadataEpoch.current) setMetadataError(userFacingError(e, t("reviewPanel.moreHistoryFailed"))); }
    finally { if (epoch === metadataEpoch.current) setHistoryBusy(false); }
  };
  useEffect(() => {
    let active = true;
    const scope = JSON.stringify([taskId, queryRootId, mode, reference, parent]);
    const sameScope = queryScope.current === scope;
    if (!sameScope) {
      queryScope.current = scope;
      setSnapshot(null); setSelection(null); setFile(null); setChanged(false);
      selectionRef.current = null;
      loadedSnapshot.current = "";
    }
    if (needsReference && (!queryRootId || !reference)) { setSnapshot(null); setBusy(false); return; }
    setBusy(true); setError("");
    void api.query({ taskId, rootId: queryRootId, mode, reference: reference || undefined, parent }).then(async next => {
      if (!active) return;
      const selected = sameScope ? selectionRef.current : null;
      const retained = selected && next.roots.some(r => r.root.id === selected.rootId && r.files.some(f => f.path === selected.path));
      // Keep the entire old snapshot visible until the selected body's new
      // frozen version is ready; then commit list, counts and body together.
      const nextFile = retained ? await api.file(taskId, next.id, selected.rootId, selected.path) : null;
      if (!active) return;
      if (retained) {
        loadedSnapshot.current = next.id;
        setFile(nextFile);
      }
      setSnapshot(next); setChanged(false);
      if (!queryRootId) {
        if (mode === "uncommitted") setRoots(next.roots.map(r => r.root));
        // The first directory may be clean or unmanaged. Choose a repository
        // with changes before any file is opened, then retain that choice so a
        // refresh cannot move an in-progress commit/push to another repository.
        setRootId(current => next.roots.some(r => r.root.id === current) ? current
          : next.roots.find(r => r.root.git && !r.error && r.files.length > 0)?.root.id ?? "");
      }
      const requested = pendingPath.current;
      const wanted = requested ? reviewTarget(next, requested) : pendingSelection.current;
      if (requested) {
        pendingPath.current = undefined;
        setTargetError(wanted ? "" : t("reviewPanel.fileNotInDiff", { path: requested }));
        if (wanted) setRootId(wanted.rootId);
      }
      pendingSelection.current = null;
      setSelection(old => { if (wanted && next.roots.some(r => r.root.id === wanted.rootId && r.files.some(f => f.path === wanted.path))) return wanted; return old && next.roots.some(r => r.root.id === old.rootId && r.files.some(f => f.path === old.path)) ? old : null; });
    }).catch(e => { if (active) { setError(userFacingError(e, t("reviewPanel.changesFailed"))); } })
      .finally(() => { if (active) setBusy(false); });
    return () => { active = false; };
  }, [api, taskId, queryRootId, mode, reference, parent, revision, needsReference]);
  useEffect(() => {
    let active = true;
    if (loadedIdentity.current !== fileIdentity) {
      setFile(null);
      loadedSnapshot.current = "";
      loadedIdentity.current = fileIdentity;
    }
    setFileError("");
    if (!selection || !snapshot || busy) return;
    if (loadedSnapshot.current === snapshot.id) return;
    void api.file(taskId, snapshot.id, selection.rootId, selection.path)
      .then(result => { if (active) { loadedSnapshot.current = snapshot.id; setFile(result); } })
      .catch(e => { if (active) setFileError(userFacingError(e, t("reviewPanel.fileDiffFailed"))); });
    return () => { active = false; };
  }, [api, taskId, selection, snapshot, busy, fileIdentity]);
  useEffect(() => {
    if (revealDiff && file && !busy) diffBody.current?.scrollIntoView?.({ behavior: "smooth", block: "nearest" });
  }, [revealDiff, file, busy]);
  const resetComparison = () => {
    pendingSelection.current = null; pendingPath.current = undefined;
    setReference(""); setParent(0); setSelection(null); setFile(null); setPreviewTarget(null); setContextMenu(null); setTargetError("");
  };
  const selectMode = (next: ReviewMode) => { resetComparison(); setMode(next); };
  const selectDirectory = (id: string) => {
    const nextRoot = roots.find(r => r.id === id);
    resetComparison(); setDirectoryId(id); setRootId(id);
    if (mode === "session" || (nextRoot ? !nextRoot.git : needsReference)) setMode("uncommitted");
  };
  const reopened = useRef(false);
  useEffect(() => { if (!visible) reopened.current = true; }, [visible]);
  const selectedCommit = commits.find(c => c.id === reference);
  const selectedRoot = snapshot?.roots.find(r => r.root.id === selection?.rootId);
  useEffect(() => {
    setUpdateError("");
    if (!visible || !snapshot || busy || running || operationBusy || mode === "committed") return;
    let active = true;
    let pending = false;
    let lastCheck = 0;
    const expected = reviewFingerprint(snapshot);
    const check = async (force = false) => {
      if (!active || pending || document.visibilityState === "hidden" || (!force && Date.now() - lastCheck < 10000)) return;
      pending = true; lastCheck = Date.now();
      try {
        const next = await api.query({ taskId, rootId: queryRootId, mode, reference: reference || undefined, parent, probe: true });
        if (!active) return;
        setUpdateError("");
        if (reviewFingerprint(next) !== expected) {
          setChanged(true);
          // Working-tree views follow external commits/staging/restores as well.
          // The normal refresh swaps list and selected body together.
          if (mode !== "branch") setRevision(value => value + 1);
        }
      } catch (cause) {
        if (active) setUpdateError(t("reviewPanel.watchFailed"));
      } finally { pending = false; }
    };
    if (reopened.current) { reopened.current = false; void check(); }
    const timer = window.setInterval(() => void check(), 10000);
    const focus = () => void check(true);
    window.addEventListener("focus", focus);
    document.addEventListener("visibilitychange", focus);
    return () => {
      active = false; window.clearInterval(timer);
      window.removeEventListener("focus", focus);
      document.removeEventListener("visibilitychange", focus);
    };
  }, [api, snapshot, taskId, queryRootId, mode, reference, parent, busy, running, operationBusy, visible]);
  const selectedChange = selectedRoot?.files.find(file => file.path === selection?.path);
  const isGitlink = selectedChange?.before?.kind === "gitlink" || selectedChange?.after?.kind === "gitlink";
  const openChange = (result: RootChanges, path: string) => {
    setPreviewTarget(null); setTargetError(""); setRootId(result.root.id);
    setSelection({ rootId: result.root.id, path });
  };
  const closePreview = () => {
    diffBody.current?.closest(".review-body")?.querySelector<HTMLButtonElement>('nav button[aria-current="true"]')?.focus();
    selectionRef.current = null;
    setSelection(null); setPreviewTarget(null); setFile(null); setFileError("");
  };
  const closePreviewButton = <button type="button" className="review-icon-button review-preview-close" aria-label={t("reviewPanel.closePreview")} data-tooltip={t("reviewPanel.closePreview")} onClick={closePreview}>
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" /></svg>
  </button>;
  useEffect(() => { setContextMenu(null); }, [snapshot?.id, tab, visible]);
  useEffect(() => { setPreviewTarget(null); }, [directoryId, mode, reference]);
  useEffect(() => {
    let active = true; setPreview(null); setPreviewError("");
    if (previewTarget) void api.preview(taskId, previewTarget.snapshotId, previewTarget.root.root.id, previewTarget.path, previewTarget.version).then(value => { if (active) setPreview(value); }).catch(cause => { if (active) setPreviewError(userFacingError(cause, t("reviewPanel.previewFailed"))); });
    return () => { active = false; };
  }, [api, taskId, previewTarget]);
  const displayRoots = snapshot?.roots;
  const listedRoots = displayRoots?.filter(r => fileView === "tree" || r.files.length > 0 || r.error || r.excludedRoots?.length);
  const emptyChanges = Boolean(displayRoots && !error && displayRoots.every(r => !r.files.length && !r.error && !r.excludedRoots?.length));
  const statistics = snapshot?.roots.flatMap(r => Object.values(r.statistics)) ?? [];
  const additions = statistics.reduce((sum, s) => sum + (s.additions ?? 0), 0);
  const deletions = statistics.reduce((sum, s) => sum + (s.deletions ?? 0), 0);
  const partialStatistics = statistics.some(s => s.error) || snapshot?.roots.some(r => r.error);
  const refreshLabel = t(busy ? "reviewPanel.reading" : changed ? "reviewPanel.changedRefresh" : "reviewPanel.refresh");
  const waitingForReference = needsReference && (!queryRootId || !reference);
  const panelLoader = useLoadingVisibility(!error && (analysisReadyTask !== taskId || ((!snapshot || snapshot.taskId !== taskId) && !waitingForReference)));
  const panelLoading = panelLoader.pending;
  return <aside className={`review-panel${standalone ? " review-standalone" : ""}${animateEntrance ? " review-panel-entering" : ""}`} aria-label={t("reviewPanel.title")} hidden={!visible} style={visible ? undefined : { display: "none" }}>
    <header className="review-titlebar" data-tauri-drag-region="deep"><WorkspaceTabs value={tab} onChange={value => { setTab(value); if (value === "files") setFilesOpened(true); if (value === "run") setRunOpened(true); }} /><div className="review-window-controls">{onDetach && <button type="button" className="review-detach" onClick={() => {
      onDetach({ tab, directoryId, rootId, mode, reference, parent, split, filter, fileView, selection, roots });
    }} aria-label={t("reviewPanel.detach")} data-tooltip={t("reviewPanel.detachTooltip")}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M7 7H5a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-2" /><rect x="9" y="3" width="12" height="12" rx="2" /><path d="M9 7h12" /></svg></button>}<button type="button" onClick={onClose} aria-label={t("reviewPanel.close")} data-tooltip={t(standalone ? "reviewPanel.closeWindow" : "reviewPanel.closeSidebar")}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><path d="m7 7 10 10M17 7 7 17" /></svg></button></div></header>
    {workspaceHeader}
    <ReviewMutationRecovery taskId={taskId} visible={visible} onChanged={() => { setRevision(value => value + 1); onWorkspaceChanged?.(); }} />
    {runOpened && <div className="review-content" role="tabpanel" aria-label={t("reviewPanel.tab.run")} hidden={tab !== "run"}><Suspense fallback={<ReviewPanelLoading />}><ProjectRunPanel key={taskId} taskId={taskId} visible={visible && tab === "run"} onEditProject={onEditProject} /></Suspense></div>}
    {filesOpened && <div className="review-content" role="tabpanel" aria-label={t("reviewPanel.tab.files")} hidden={tab !== "files"}><WorkspaceFiles key={taskId} taskId={taskId} visible={visible && tab === "files"} reveal={treeRequest} onEditProject={onEditProject} /></div>}
    <div className="review-content" role="tabpanel" aria-label={t("reviewPanel.tab.changes")} hidden={tab !== "changes"}>
    <div className="review-controls">
      {roots.length > 1 && <select className="review-directory-select" aria-label={t("reviewPanel.compareDirectory")} data-tooltip={scopeRoot?.path ?? t("reviewPanel.allDirectories")} value={directoryId} disabled={operationBusy} onChange={e => selectDirectory(e.target.value)}>
        <option value="">{t("reviewPanel.allDirectories")}</option>
        {roots.map(r => <option key={r.id} value={r.id} data-tooltip={r.path}>{roots.filter(other => directoryName(other.path) === directoryName(r.path)).length > 1 ? r.path : directoryName(r.path)}{!r.git ? t("reviewPanel.noGitSuffix") : ""}</option>)}
      </select>}
      {hasGit ? <select className="review-mode-select" aria-label={t("reviewPanel.compareMode")} data-tooltip={modeLabels[mode]} value={mode} disabled={operationBusy} onChange={e => selectMode(e.target.value as ReviewMode)}>{modes.map(m => <option key={m} value={m}>{modeLabels[m]}</option>)}</select> : roots.length > 0 && <span className="review-session-label" data-tooltip={t("reviewPanel.sessionTooltip")}>{t("reviewPanel.mode.session")}</span>}
      {needsReference && <select className="review-reference-select" aria-label={t(mode === "committed" ? "reviewPanel.compareCommit" : "reviewPanel.targetBranch")} data-tooltip={selectedCommit?.subject ?? (reference || t(mode === "committed" ? "reviewPanel.chooseCommit" : "reviewPanel.chooseBranch"))} value={reference} onChange={e => { setReference(e.target.value); setParent(0); }}><option value="">{t(mode === "committed" ? "reviewPanel.chooseCommit" : "reviewPanel.chooseBranch")}</option>{mode === "committed" ? commits.map(c => <option key={c.id} value={c.id}>{c.subject}</option>) : branches.map(b => <option key={b.name} value={b.name}>{b.name}</option>)}</select>}
      {selectedCommit && selectedCommit.parents.length > 1 && <select className="review-parent-select" aria-label={t("reviewPanel.parentCommit")} data-tooltip={t("reviewPanel.parent", { index: parent + 1 })} value={parent} onChange={e => setParent(Number(e.target.value))}>{selectedCommit.parents.map((p, i) => <option key={p} value={i}>{t("reviewPanel.parent", { index: i + 1 })}</option>)}</select>}
      {hasMore && <button className="review-icon-button review-more-history" type="button" aria-label={t("reviewPanel.moreHistory")} data-tooltip={t(historyBusy ? "reviewPanel.reading" : "reviewPanel.moreHistory")} disabled={historyBusy} onClick={() => void moreHistory()}><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M3 3h10M3 6h10m-5 3v5m-3-3 3 3 3-3" /></svg></button>}
    </div>
    <div className="review-changes-body">
    {panelLoading && <ReviewPanelLoading showControls={false} visible={panelLoader.visible} />}
    <div className="review-changes-content" data-loading={panelLoading || undefined} inert={panelLoading} aria-hidden={panelLoading || undefined}>
    {targetError && <InlineError role="status">{targetError}</InlineError>}
    <ReviewDiffSplit expanded={expandedReviewTask === taskId} review={<ProjectReviewActions key={taskId} taskId={taskId} snapshot={snapshot} disabled={busy || operationBusy} visible={visible && tab === "changes"} client={analysisClient}
      onExpandedChange={onReviewExpanded} onInitialLoad={onAnalysisReady}
      onReviewFinding={finding => {
        const target = snapshot && reviewFindingTarget(snapshot, finding);
        const result = target && snapshot?.roots.find(r => r.root.id === target.rootId);
        if (!target || !result) { setTargetError(t("reviewPanel.fileNotInScope")); return; }
        openChange(result, target.path); setRevealDiff(value => value + 1);
      }}/> }>
    {error && <InlineError onRetry={busy ? undefined : () => setRevision(n => n + 1)} retryLabel={t("reviewPanel.refresh")}>{error}</InlineError>}{metadataError && <InlineError>{metadataError}</InlineError>}
    {updateError && <InlineError role="status">{updateError}</InlineError>}
    <div className="review-sidebar-header"><span className="review-sidebar-title">{t("reviewPanel.files")}{snapshot && (additions > 0 || deletions > 0 || partialStatistics) && <span className="review-total" data-tooltip={partialStatistics ? t("reviewPanel.partialStatisticsTooltip") : undefined}><span className="review-add-count">+{additions}</span><span className="review-delete-count">−{deletions}</span>{partialStatistics && <small>{t("reviewPanel.partialStatistics")}</small>}</span>}</span><span className="review-sidebar-actions"><button type="button" className="review-view-toggle review-icon-button" aria-label={t(fileView === "list" ? "reviewPanel.showTree" : "reviewPanel.showList")} data-tooltip={t(fileView === "list" ? "reviewPanel.showTree" : "reviewPanel.showList")} onClick={() => setFileView(current => current === "list" ? "tree" : "list")}><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" aria-hidden="true">{fileView === "list" ? <path d="M1 2h14M4 2v11h11M4 6h8M4 9h11" /> : <path d="M2 3h12M2 6h12M2 9h12M2 12h8" />}</svg></button><button type="button" className="review-icon-button review-refresh" aria-label={t("reviewPanel.refresh")} data-tooltip={refreshLabel} aria-busy={busy} data-changed={changed || undefined} disabled={busy || operationBusy} onClick={() => setRevision(n => n + 1)}><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M13 6a5 5 0 0 0-8.5-2L2 6m0-4v4h4m-3 4a5 5 0 0 0 8.5 2l2.5-2m0 4v-4h-4" /></svg></button><button type="button" className="review-icon-button review-split-toggle" aria-label={t("reviewPanel.split")} data-tooltip={t(split ? "reviewPanel.unified" : "reviewPanel.split")} aria-pressed={split} onClick={() => setSplit(value => !value)}><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="2" y="2" width="12" height="12" rx="2" /><path d="M8 2v12M4 5h1m-1 3h1m6-3h1m-1 3h1" /></svg></button><ReviewGitMenu key={`${taskId}:${root?.id}`} taskId={taskId} root={root} disabled={running || busy || operationBusy} visible={visible && tab === "changes"} onBusy={setOperationBusy} onChanged={() => setRevision(value => value + 1)} onCompleted={action => { setPulledRevision(value => value + 1); if (action === "sync") setSyncedRevision(value => value + 1); }} onNotice={setRemoteNotice} /></span></div>
    <ReviewGitRemoteStatus notice={remoteNotice?.rootId === root?.id ? remoteNotice : null} />
    <ReviewGitActions key={`${taskId}:${root?.id}`} taskId={taskId} root={root} syncedRevision={syncedRevision} pulledRevision={pulledRevision} snapshotId={snapshot?.id ?? ""} hasChanges={Boolean(snapshot?.roots.some(result => result.root.id === root?.id && result.root.git && !result.error && ["uncommitted", "unstaged", "staged"].includes(result.mode) && result.files.length > 0))} disabled={busy || operationBusy || changed} onBusy={setOperationBusy} onChanged={() => setRevision(n => n + 1)} />
    {snapshot && !emptyChanges && <ReviewSelectionActions key={selectionScope} taskId={taskId} snapshot={snapshot} selected={fileSelection.selected} disabled={running || busy || operationBusy} api={api} onBusy={setOperationBusy} onChanged={() => setRevision(value => value + 1)} onRemove={fileSelection.remove} menu={visible && tab === "changes" ? contextMenu : null} onCloseMenu={closeContextMenu} />}
    <ReviewFilesSplit active={Boolean(selection || previewTarget)} busy={busy} files={
      <nav ref={fileSelection.nav} tabIndex={0} aria-label={t("reviewPanel.fileList")} className={emptyChanges ? "review-empty-list" : "review-selectable-list"} onPointerDown={fileSelection.onPointerDown} onClickCapture={fileSelection.onClickCapture} onKeyDown={fileSelection.onKeyDown} onDragStart={event => event.preventDefault()}>{emptyChanges ? <ReviewEmptyState description={emptyDescriptions[mode]} /> : listedRoots?.map(r => {
        return <ReviewFileTree key={r.root.id} view={fileView} showRoot={(displayRoots?.length ?? 0) > 1} result={r} filter={filter} busy={busy || operationBusy} selectedFiles={fileSelection.selected} active={root?.id === r.root.id} onActivate={() => { if (operationBusy) return; setRootId(r.root.id); if (selection?.rootId !== r.root.id) setSelection(null); }} selectedPath={previewTarget?.root.root.id === r.root.id ? previewTarget.path : selection?.rootId === r.root.id ? selection.path : undefined}
        onSelect={path => { if (!operationBusy) openChange(r, path); }} onContextMenu={(file, point) => { if (busy || operationBusy) return; fileSelection.contextSelect(r.root.id, file.path); setContextMenu({ root: r, file, snapshotId: snapshot!.id, ...point }); }} />;
      })}{fileSelection.rectangle && <div className="review-selection-rectangle" style={fileSelection.rectangle} aria-hidden="true" />}</nav>}>
      {(selection || previewTarget) && <div className="review-file-body" ref={diffBody}>{previewTarget ? <><div className="review-file-heading"><strong data-tooltip={`${previewTarget.root.root.path}/${previewTarget.path}`}>{directoryName(previewTarget.root.root.path)}/{previewTarget.path}{previewTarget.version === "head" ? " (HEAD)" : ""}</strong>{closePreviewButton}</div>{previewError ? <InlineError>{previewError}</InlineError> : preview ? <WorkspaceFilePreview preview={preview} path={previewTarget.path} /> : <ReviewFileLoading />}</> : selection ? <><div className="review-file-heading"><strong data-tooltip={`${directoryName(selectedRoot?.root.path ?? "")}/${selection.path}`}>{directoryName(selectedRoot?.root.path ?? "")}/{selection.path}</strong><span className="review-file-heading-actions">{!fileError && !isGitlink && !file?.binary && file?.text && <span className="review-entry-counts"><span className="review-add-count">+{file.text.additions}</span><span className="review-delete-count">−{file.text.deletions}</span></span>}{closePreviewButton}</span></div>{fileError ? <InlineError>{fileError}</InlineError> : isGitlink ? <p className="review-tree-message">{t("reviewPanel.submoduleChanged")}</p> : file?.binary || file?.beforeImage || file?.afterImage ? <ReviewBinaryPlaceholder beforeImage={file.beforeImage} afterImage={file.afterImage} /> : file?.text ? <ReviewText key={`${selection.rootId}:${selection.path}:${mode}:${reference}`} diff={file.text} path={selection.path} split={split} showStatistics={false} /> : <ReviewFileLoading split={split} />}</> : null}</div>}
    </ReviewFilesSplit>
    </ReviewDiffSplit>
    </div>
    </div>
    </div>
    {contextMenu && fileSelection.selected.size <= 1 && visible && tab === "changes" && <ReviewFileMenu key={`${contextMenu.snapshotId}:${contextMenu.root.root.id}:${contextMenu.file.path}:${contextMenu.x}:${contextMenu.y}`} target={contextMenu} taskId={taskId} disabled={running || busy || operationBusy} api={api} onClose={() => setContextMenu(null)} onBusy={setOperationBusy} onChanged={() => setRevision(value => value + 1)}
      onOpenChanges={() => openChange(contextMenu.root, contextMenu.file.path)} onOpenFile={version => { setTargetError(""); setPreviewTarget({ root: contextMenu.root, path: contextMenu.file.path, snapshotId: contextMenu.snapshotId, version }); }}
      onRevealTree={() => { setTreeRequest(previous => ({ path: `${contextMenu.root.root.path}/${contextMenu.file.path}`, id: (previous?.id ?? 0) + 1 })); setFilesOpened(true); setTab("files"); }} />}
  </aside>;
}
