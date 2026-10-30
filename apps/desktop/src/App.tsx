import { RemoteDevices } from "./RemoteDevices";
import { invoke } from "@tauri-apps/api/core";
import { AttachmentUploadRecovery } from "./AttachmentUploadRecovery";
import {
  type UiLanguage,
  type ResolvedLanguage,
  SUPPORTED_LANGUAGES,
  resolveLanguage,
  getPreferenceLabels,
  translate as t,
  I18nProvider,
} from "./i18n";
import { ulid } from "ulid";
import { SubmissionRecovery, usePendingSubmissions, useUnconfirmedSubmissions } from "./SubmissionRecovery";
import { PendingSubmissionError, UnconfirmedSubmissionError, submissionOperations } from "./submissionOperations";
import { ApprovalRecovery, usePendingApprovalResponses } from "./ApprovalRecovery";
import { approvalOperations } from "./approvalOperations";
import { ContextUsageRing } from "./ContextUsageRing";
import { CancelRecovery, usePendingCancellations } from "./CancelRecovery";
import { cancelOperations } from "./cancelOperations";
import { enabledAgents, orderAgents, useAgentPreferences, moveAgent, type AgentPreferences } from "./agentPreferences";
import { AgentManagementControls, AGENT_DRAG_TYPE } from "./AgentManagementControls";
import { AgentLogin, agentAccountIssue, needsAgentLogin } from "./AgentLogin";
import "./agentManagement.css";
import { LoadingStatus } from "./LoadingStatus";
import { revealLocalProject } from "./localProject";
import { AgentPickerMenu } from "./AgentPickerMenu";
import { focusComposerInput } from "./composerFocus";
import { StartupShell } from "./StartupShell";
import { StartupRecovery } from "./StartupGate";
import { userFacingError } from "./userFacingError";
import { AgentQuotaContext } from "./AgentQuotaRing";
import { AgentIcon, providerLabel } from "./agentPresentation";
import { SidebarWindowControls } from "./SidebarWindowControls";
import { SidebarNavIcon } from "./SidebarNavIcon";
import { ActivityIcon } from "./ActivityIcon";
import { useConversationNavigation } from "./useConversationNavigation";
import { useConversationReview } from "./useConversationReview";
import type { PendingAgent } from "./AgentPending";
import { AgentUpdateControl, useAgentUpdates, type AgentUpdateActions } from "./agentUpdates";
import { useComposerLabels } from "./useComposerLabels";
import { traceStartup } from "./startupTiming";
import { FloatingPopover } from "./FloatingPopover";
import { AgentWelcome, InstallationLink, supportedAgents } from "./AgentWelcome";
import { SidebarResizeHandle, useSidebarWidth } from "./SidebarResizeHandle";
import { ReviewResizeHandle, useReviewWidth } from "./ReviewResizeHandle";
import { AttachmentPreviews, SentAttachments, useAttachments } from "./attachments";
import { visibleQuotaViews } from "./quotaInstallations";
import { useModelReadyExecutors } from "./useModelReadyExecutors";
import { ModelCatalogCache } from "./modelCatalogCache";
import { useModelCatalog } from "./useModelCatalog";
import { DropdownChevron } from "./DropdownChevron";
import { PermissionPicker } from "./PermissionPicker";
import { ModelPicker } from "./ModelPicker";
import { useReviewWindow } from "./reviewWindow";
import { ConversationLoading } from "./ConversationLoading";
import { StreamingTimeline, isTimelineDelta } from "./streamingTimeline";
import { ConversationPages } from "./ConversationPages";
import { StreamingMarkdownMessage } from "./MarkdownMessage";
import { ConversationContent, SubagentPanel, activityOf } from "./ConversationActivity";
import { lazy, memo, Suspense, Fragment, type ReactNode, type RefObject, FormEvent, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { adjustUiScale, parseUiScale, scaleShortcut, MIN_UI_SCALE, MAX_UI_SCALE, type ScaleAction } from "./uiScale";
import type {
  BootstrapSnapshot,
  PendingApproval,
  SubmissionResolution,
  QuotaSnapshotView,
  CoreClient,
  EventStreamMessage,
  ExecutorRef,
  InstallationSummary,
  InstallationStatus,
  TaskId,
  ProjectId,
  ProjectSummary,
  TaskSummary,
  TimelineItem,
} from "@dogcodes/protocol";
import { ProjectPicker } from "./ProjectPicker";
import { ProjectDialog } from "./ProjectDialog";
import { DemoCoreClient } from "./demoClient";
import "./terminal.css";
const TerminalDock = lazy(() => import("./TerminalDock").then(module => ({ default: module.TerminalDock })));
import { ConversationReviewDocks } from "./ReviewPanel";
import { ApprovalCard } from "./ApprovalCard";
import { TauriCoreClient } from "./tauriClient";
import { OrderedEventCursor } from "./orderedEvents";
import { PendingExecutorContext } from "./automaticContext";
import { ConversationHistory } from "./ConversationHistory";
const MemoConversationHistory = memo(ConversationHistory);
import { timelineItemRole } from "./timelinePresentation";
import { RequestEpoch, changeSelection } from "./requestEpoch";
import { formatResetTime, quotaScopeLabel, quotaPlanLabel, quotaNativeResetLabel, nativeResetDisplay, antigravityQuotaForModel, ringQuotaWindow, quotaColor, quotaIsExhausted, quotaIsStale, quotaWindowHasReset, quotaWindowLabel, remainingPercent } from "./quotaPresentation";
import { createQuotaRefresh, type QuotaRefreshProgress } from "./quotaRefresh";
import { useLoadingVisibility } from "./useLoadingVisibility";
import { quotaShowsLastReading } from "./quotaFallback";
import { currentAgentExecutor, readyAgentInstallations } from "./agentOptions";



const defaultClient: CoreClient = "__TAURI_INTERNALS__" in window ? new TauriCoreClient() : new DemoCoreClient();
const appLogo = new URL("../src-tauri/icons/128x128.png", import.meta.url).href;
const LAST_CONVERSATION_KEY = "dogcodes.conversation.lastViewed.v1";

function executorKey(executor: ExecutorRef | null | undefined): string {
  return executor ? `${executor.providerId}:${executor.installationId ?? "default"}` : "";
}

function executorLabel(executor: ExecutorRef | null): string {
  if (!executor) return "加载中";
  return executor.installationId ? `${executor.providerId} (${executor.installationId.slice(0, 10)}…)` : executor.providerId;
}

function defaultExecutor(snapshot: BootstrapSnapshot, preferences: AgentPreferences): ExecutorRef {
  const installation = enabledAgents(readyAgentInstallations(snapshot.installations), item => item.providerId, preferences)[0];
  return installation ? { providerId: installation.providerId, installationId: installation.installationId } : { providerId: "fake", installationId: null };
}

function projectLabel(name: string): string {
  return name === "快速对话" ? "无项目" : name;
}

function ToolActivity({ item, active }: { item: TimelineItem; active: boolean }) {
  const failed = item.kind === "tool.failed";
  const running = active && item.kind === "tool.started";
  return (
    <article className={`tool-activity${running ? " running activity-running" : failed ? "" : " completed"}`}>
      <ActivityIcon kind="工具" />
      <span className="activity-text">{item.text}</span>
      {!failed && <small><span className="activity-text">{running ? "进行中" : item.kind === "tool.started" ? "结果未确认" : "完成"}</span></small>}
    </article>
  );
}

const TimelineEntry = memo(function TimelineEntry({ item, active = false }: { item: TimelineItem; active?: boolean }) {
  const role = timelineItemRole(item.kind);
  const time = new Date(item.occurredAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (role === "tool") return <ToolActivity item={item} active={active} />;
  if (role === "user") {
    return (
      <article className="message user-message" data-tooltip={`你 · ${time}`}>
        {item.text && <p>{item.text}</p>}
        <SentAttachments items={item.attachments} />
      </article>
    );
  }
  if (role === "assistant") {
    return (
      <article className="message assistant-message" aria-label={`${providerLabel(item.providerId ?? "dogcodes")} · ${time}`}>
        {item.text ? <StreamingMarkdownMessage text={item.text} streaming={active} /> : <span className="typing">正在生成</span>}
      </article>
    );
  }
  return (
    <article className={`message system-message ${item.kind}`}>
      <div className="avatar"><AgentIcon providerId={item.providerId ?? "dogcodes"} /></div>
      <div>
        <header><b>{providerLabel(item.providerId ?? "dogcodes")}</b><time>{time}</time></header>
        <p>{item.text}</p>
      </div>
    </article>
  );
});

type ExecutorOption = {
  key: string;
  executor: ExecutorRef;
  quota: QuotaSnapshotView["quota"];
};

type UiTheme = "system" | "light" | "dark";
type PreferencesSection = "language" | "theme" | "scale" | null;

function readPreference<T extends string>(key: string, fallback: T): T {
  if (typeof window === "undefined") return fallback;
  try {
    const value = window.localStorage.getItem(key);
    return value === null ? fallback : value as T;
  } catch {
    return fallback;
  }
}

function isEnglishLanguage(language: UiLanguage): boolean {
  return language === "en" || (language === "system" && typeof navigator !== "undefined" && navigator.language.startsWith("en"));
}

function isMacPlatform(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
}

function PreferencesIcon({ kind }: { kind: Exclude<PreferencesSection, null> }) {
  return <svg className="preferences-icon" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {kind === "language" ? <>
      <circle cx="10" cy="10" r="7" />
      <path d="M3 10h14M10 3c2 2.2 2.8 4.5 2.8 7S12 14.8 10 17c-2-2.2-2.8-4.5-2.8-7S8 5.2 10 3Z" />
    </> : kind === "theme" ? <>
      <circle cx="10" cy="10" r="7" />
      <path d="M10 3a7 7 0 0 1 0 14Z" fill="currentColor" />
    </> : <>
      <circle cx="9" cy="9" r="5.5" />
      <path d="m13 13 4 4M7 9h4M9 7v4" />
    </>}
  </svg>;
}

function PreferencesMenu({
  open,
  section,
  language,
  theme,
  scale,
  onToggle,
  onLanguage,
  onTheme,
  onScale,
  onRestartCore,
  anchorRef,
  popoverRef,
}: {
  anchorRef: RefObject<HTMLElement | null>;
  popoverRef: RefObject<HTMLDivElement | null>;
  open: boolean;
  section: PreferencesSection;
  language: UiLanguage;
  theme: UiTheme;
  scale: number;
  onToggle: (section: Exclude<PreferencesSection, null>) => void;
  onLanguage: (value: UiLanguage) => void;
  onTheme: (value: UiTheme) => void;
  onScale: (value: ScaleAction) => void;
  onRestartCore: () => void;
}) {
  const labels = getPreferenceLabels(language);
  const modifier = isMacPlatform() ? "⌘" : "Ctrl";
  const [position, setPosition] = useState<{ left: number; bottom: number; width: number; maxHeight: number } | null>(null);
  // Float above the footer row in the document layer, never inside the footer itself.
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const anchor = anchorRef.current?.getBoundingClientRect();
      if (!anchor) return;
      const margin = 12;
      const width = Math.min(Math.max(200, anchor.width), window.innerWidth - margin * 2);
      const bottom = Math.max(margin, window.innerHeight - anchor.top + 6);
      setPosition({
        left: Math.max(margin, Math.min(anchor.left, window.innerWidth - width - margin)),
        bottom,
        width,
        maxHeight: Math.max(0, window.innerHeight - bottom - margin),
      });
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [open, anchorRef, scale]);
  if (!open || !position) return null;

  const optionMenu = section === "language" ? (
    <div className="preferences-submenu" role="menu" aria-label={labels.language}>
      {SUPPORTED_LANGUAGES.map((item) => (
        <button key={item.id} type="button" role="menuitemradio" aria-checked={language === item.id} onClick={() => onLanguage(item.id)}>
          <span>{item.nativeLabel}</span>{language === item.id && <span aria-hidden="true">✓</span>}
        </button>
      ))}
    </div>
  ) : section === "theme" ? (
    <div className="preferences-submenu" role="menu" aria-label={labels.theme}>
      {([
        ["system", labels.system],
        ["light", labels.light],
        ["dark", labels.dark],
      ] as const).map(([value, label]) => (
        <button key={value} type="button" role="menuitemradio" aria-checked={theme === value} onClick={() => onTheme(value)}>
          <span>{label}</span>{theme === value && <span aria-hidden="true">✓</span>}
        </button>
      ))}
    </div>
  ) : section === "scale" ? (
    <div className="preferences-submenu" role="menu" aria-label={labels.scale}>
      {([
        ["in", labels.zoomIn, "+", scale >= MAX_UI_SCALE],
        ["out", labels.zoomOut, "−", scale <= MIN_UI_SCALE],
        ["reset", labels.actualSize, "0", scale === 100],
      ] as const).map(([value, label, key, disabled]) => (
        <button key={value} type="button" role="menuitem" disabled={disabled} onClick={() => onScale(value)}>
          <span>{label}</span><small>{modifier} {key}</small>
        </button>
      ))}
    </div>
  ) : null;

  return createPortal(
    <div ref={popoverRef} className="preferences-popover" role="menu" style={{ left: position.left, bottom: position.bottom, width: position.width, "--preferences-menu-max-height": `${position.maxHeight}px` } as React.CSSProperties}>
      <button type="button" role="menuitem" className={section === "language" ? "selected" : ""} onClick={() => onToggle("language")}>
        <PreferencesIcon kind="language" /><span>{labels.language}</span><span aria-hidden="true">›</span>
      </button>
      <button type="button" role="menuitem" className={section === "theme" ? "selected" : ""} onClick={() => onToggle("theme")}>
        <PreferencesIcon kind="theme" /><span>{labels.theme}</span><span aria-hidden="true">›</span>
      </button>
      <button type="button" role="menuitem" className={section === "scale" ? "selected" : ""} onClick={() => onToggle("scale")}>
        <PreferencesIcon kind="scale" /><span>{labels.scale}</span><span aria-hidden="true">›</span>
      </button>
      {optionMenu}
      {"__TAURI_INTERNALS__" in window && <button type="button" role="menuitem" className="background-service-menuitem" onClick={onRestartCore}>
        <svg className="preferences-icon" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><path d="M16 9a6 6 0 1 0-1 4M16 4v5h-5" /></svg>
        <span>{t("backgroundService.restart", undefined, resolveLanguage(language))}</span>
      </button>}
    </div>,
    document.body,
  );
}

export function App({ client = defaultClient }: { client?: CoreClient } = {}) {
  const agentPreferences = useAgentPreferences();
  useEffect(() => { traceStartup("app_mounted"); }, []);
  const modelCache = useMemo(() => new ModelCatalogCache((executor, model, taskId) => client.availableModels ? client.availableModels(executor, model, taskId) : Promise.resolve({models:[],defaultModel:null})), [client]);
  const [welcomeOpen, setWelcomeOpen] = useState(() => {
    // Explicitly injected clients are embedded hosts; the default app owns first-run UX.
    if (client !== defaultClient) return false;
    try { return localStorage.getItem("dogcodes.welcome.completed.v1") !== "true"; } catch { return true; }
  });
  const closeWelcome = () => {
    try { localStorage.setItem("dogcodes.welcome.completed.v1", "true"); } catch { /* Storage can be disabled. */ }
    setWelcomeOpen(false);
    void refresh();
    refreshQuotas.current();
  };
  const detectAgents = async (providerIds: string[]) => {
    // A failed scan leaves that agent listed as not installed; the others still update.
    await Promise.all(providerIds.map(id => client.scanAgent(id).catch(() => null)));
    await refresh();
    refreshQuotas.current();
  };
  const [activeView, setActiveView] = useState<"conversation" | "quota" | "remote">("conversation");
  const [startupError, setStartupError] = useState(false);
  const [snapshot, setSnapshot] = useState<BootstrapSnapshot | null>(null);
  const [selectedTaskId, setSelectedTaskId] = useState<TaskId | null>(null);
  const pendingCancellations = usePendingCancellations(selectedTaskId);
  const pendingApprovalResponses = usePendingApprovalResponses(selectedTaskId);
  const cancelGuard = useRef(false);
  const navigationTaskIds = useMemo(() => snapshot?.tasks.map(task => task.taskId) ?? [], [snapshot?.tasks]);
  const navigation = useConversationNavigation(selectedTaskId, navigationTaskIds);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => readPreference<"true" | "false">("dogcodes.sidebar.collapsed", "false") === "true");
  const [draftProjectId, setDraftProjectId] = useState<ProjectId | null>(null);
  const [draftExecutor, setDraftExecutor] = useState<ExecutorRef | null>(null);
  const [timeline, setTimeline] = useState<TimelineItem[]>([]);
  const conversationCache = useRef(new Map<TaskId, { items: TimelineItem[]; dirty: boolean }>());
  const timelineStreams = useRef(new Map<TaskId, StreamingTimeline>());
  const streamVersions = useRef(new Map<TaskId, number>());
  const latestTask = useCallback((task: TaskSummary): TaskSummary => {
    const streamVersion = Math.max(task.streamVersion, streamVersions.current.get(task.taskId) ?? 0);
    return streamVersion === task.streamVersion ? task : { ...task, streamVersion };
  }, []);
  const [draft, setDraftValue] = useState("");
  const draftRevision = useRef(0), composerContext = useRef(0);
  const newDraftScope = useRef(`draft:${ulid()}`);
  const setDraft = useCallback((value: string | ((previous: string) => string)) => {
    draftRevision.current++;
    setDraftValue(value);
  }, []);
  const pendingSends = usePendingSubmissions(client);
  const unconfirmedSends = useUnconfirmedSubmissions();
  // A send whose outcome is unknown blocks a fresh send of the same draft until verified.
  const submissionPending = !Array.isArray(pendingSends) || pendingSends.some(record => selectedTaskId
    ? record.task.taskId === selectedTaskId : record.scope === newDraftScope.current)
    || unconfirmedSends.records.some(record => selectedTaskId ? record.taskId === selectedTaskId : record.scope === newDraftScope.current);
  const attachments = useAttachments(client);
  const clearAttachments = attachments.clear;
  const [draggingFiles, setDraggingFiles] = useState(false);
  const dragDepth = useRef(0);
  const attachmentInput = useRef<HTMLInputElement>(null);
  const [permissionSelection,setPermissionSelection] = useState<{key:string;mode:string|null}|null>(null);
  const [newSessionModel, setNewSessionModel] = useState<{ key: string; model: string | null; thinking: string | null } | null>(null);
  const submittedSelections = useRef(new Map<TaskId, typeof newSessionModel>());
  const [sending, setSending] = useState(false);
  const [pendingSubmission, setPendingSubmission] = useState<{scope: string; taskId: TaskId; providerId: string; operationId?: string; acceptedSequence?: number; message: TimelineItem} | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [terminalOpen, setTerminalOpen] = useState(false);
  const [terminalLoaded, setTerminalLoaded] = useState(false);
  const [reviewOpen, setReviewOpen] = useConversationReview(selectedTaskId);
  const sidebarWidth = useSidebarWidth();
  const reviewWidth = useReviewWidth(sidebarCollapsed ? 0 : sidebarWidth.width);
  const [reviewFileRequest, setReviewFileRequest] = useState<{ taskId: string; path: string; id: number } | null>(null);
  const reviewWindow = useReviewWindow(selectedTaskId, () => setReviewOpen(true));
  const [subagentItemId, setSubagentItemId] = useState<string | null>(null);
  const openSubagent = useCallback((id: string) => { setSubagentItemId(id); setReviewOpen(false); }, [setReviewOpen]);
  const [editingProject, setEditingProject] = useState<ProjectSummary | null>(null);
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const creatingTask = sending && selectedTaskId === null;
  const [error, setError] = useState<string | null>(null);
  const switchBusy = useRef(false);
  const [nextExecutors, setNextExecutors] = useState<Record<string, ExecutorRef>>({});
  const [pendingApprovals, setPendingApprovals] = useState<PendingApproval[]>([]);
  const [quotaViews, setQuotaViews] = useState<QuotaSnapshotView[]>([]);
  const [quotaTaskId, setQuotaTaskId] = useState<TaskId | null>(null);
  const agentUpdates = useAgentUpdates(client, snapshot?.installations ?? [], setSnapshot);
  const [approving, setApproving] = useState<string | null>(null);
  const [approvalError, setApprovalError] = useState<{ taskId: TaskId; approval: PendingApproval; message: string } | null>(null);
  const approvalBusy = useRef(false);
  const selectedTaskIdRef = useRef<TaskId | null>(null);
  const previousTasks = useRef<TaskSummary[]>([]);
  const initialSelectionHandled = useRef(false);
  const timelineEpoch = useRef(new RequestEpoch());
  const approvalEpoch = useRef(new RequestEpoch());
  const quotaEpoch = useRef(new RequestEpoch());
  const refreshQuotas = useRef<() => void>(() => {});
  const refreshModelCatalog = useRef<() => void>(() => {});
  const refreshQuotasNow = useRef<() => void>(() => {});
  const refreshSessionUsage = useRef<(id: string, provider: string) => void>(() => {});
  const [quotaRefreshProgress, setQuotaRefreshProgress] = useState<QuotaRefreshProgress | null>(null);
  const [timelineLoading, setTimelineLoading] = useState(false);
  const [loadedTaskId, setLoadedTaskId] = useState<TaskId | null>(null);
  const [timelineError, setTimelineError] = useState<string | null>(null);
  const [preferencesOpen, setPreferencesOpen] = useState(false);
  const [preferencesSection, setPreferencesSection] = useState<PreferencesSection>(null);
  const [uiLanguage, setUiLanguage] = useState<UiLanguage>(() => readPreference("dogcodes.ui.language", "system"));
  const resolvedLang = resolveLanguage(uiLanguage);
  const native = "__TAURI_INTERNALS__" in window;
  useEffect(() => {
    if (!native) return;
    void invoke("desktop_close_labels", { labels: {
      title: t("backgroundService.closeTitle", undefined, resolvedLang),
      warning: t("backgroundService.closeWarning", undefined, resolvedLang),
      confirm: t("backgroundService.closeConfirm", undefined, resolvedLang),
      cancel: t("common.cancel", undefined, resolvedLang),
    } }).catch(() => {});
  }, [native, resolvedLang]);
  const restartBackground = () => {
    setPreferencesOpen(false); setPreferencesSection(null);
    void invoke("core_service_restart", { labels: {
      title: t("backgroundService.confirmTitle", undefined, resolvedLang),
      warning: t("backgroundService.warning", undefined, resolvedLang),
      confirm: t("backgroundService.restart", undefined, resolvedLang),
      cancel: t("common.cancel", undefined, resolvedLang),
    }, failure: t("backgroundService.failed", undefined, resolvedLang) }).catch(() => {});
  };
  const labels = getPreferenceLabels(uiLanguage);
  const [uiTheme, setUiTheme] = useState<UiTheme>(() => readPreference("dogcodes.ui.theme", "system"));
  const [uiScale, setUiScale] = useState(() => parseUiScale(readPreference("dogcodes.ui.scale", "100")));
  const preferencesRef = useRef<HTMLDivElement | null>(null);
  const preferencesPopoverRef = useRef<HTMLDivElement | null>(null);
  const newConversationButton = useRef<HTMLButtonElement | null>(null);
  const quotaButton = useRef<HTMLButtonElement | null>(null);
  const shortcutModifier = isMacPlatform() ? "⌘" : "Ctrl";
  const shortcutKey = isMacPlatform() ? "Meta" : "Control";

  useEffect(() => {
    const navigate = (event: KeyboardEvent) => {
      const modifier = isMacPlatform() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (!modifier || event.altKey || event.shiftKey || event.isComposing || event.defaultPrevented) return;
      const key = event.key.toLowerCase();
      const target = key === "n" ? newConversationButton.current : key === "u" ? quotaButton.current : null;
      if (!target) return;
      event.preventDefault();
      // A shortcut must not dismiss a confirmation or bypass a disabled action.
      if (event.repeat || target.disabled || document.querySelector('dialog[open], [role="dialog"][aria-modal="true"], [role="alertdialog"][aria-modal="true"]')) return;
      target.click();
    };
    window.addEventListener("keydown", navigate, true);
    return () => window.removeEventListener("keydown", navigate, true);
  }, []);

  useEffect(() => {
    const root = document.documentElement;
    root.dataset.sidebarCollapsed = String(sidebarCollapsed);
    root.dataset.theme = uiTheme === "system" ? "" : uiTheme;
    root.style.setProperty("--ui-scale", String(uiScale / 100));
    const resolved = resolveLanguage(uiLanguage);
    root.lang = resolved;
    try {
      window.localStorage.setItem("dogcodes.ui.language", uiLanguage);
      window.localStorage.setItem("dogcodes.ui.theme", uiTheme);
      window.localStorage.setItem("dogcodes.ui.scale", String(uiScale));
    } catch {
      // Preferences still apply for the current session when storage is unavailable.
    }
  }, [uiLanguage, uiTheme, uiScale, sidebarCollapsed]);

  useEffect(() => {
    if (!preferencesOpen) return;
    const close = () => { setPreferencesOpen(false); setPreferencesSection(null); };
    const inside = (target: EventTarget | null) => target instanceof Node
      && (!!preferencesRef.current?.contains(target) || !!preferencesPopoverRef.current?.contains(target));
    // Capture phase: elements that stop propagation (drag regions, rows) still close it.
    const closeOnOutsidePointer = (event: PointerEvent) => { if (!inside(event.target)) close(); };
    // Keyboard focus moving elsewhere also ends the menu.
    const closeOnOutsideFocus = (event: FocusEvent) => { if (!inside(event.target)) close(); };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setPreferencesOpen(false);
      setPreferencesSection(null);
      preferencesRef.current?.querySelector<HTMLButtonElement>(".preferences-trigger")?.focus();
    };
    document.addEventListener("pointerdown", closeOnOutsidePointer, true);
    document.addEventListener("focusin", closeOnOutsideFocus);
    document.addEventListener("keydown", closeOnEscape);
    // Clicking outside the app window (or switching apps) blurs the window.
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer, true);
      document.removeEventListener("focusin", closeOnOutsideFocus);
      document.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("blur", close);
    };
  }, [preferencesOpen]);

  useEffect(() => {
    const changeScale = (event: KeyboardEvent) => {
      const action = scaleShortcut(event);
      if (action) {
        event.preventDefault();
        setUiScale(current => adjustUiScale(current, action));
      }
    };
    window.addEventListener("keydown", changeScale, true);
    return () => window.removeEventListener("keydown", changeScale, true);
  }, []);

  const selectTask = useCallback((taskId: TaskId | null, recordVisit = true) => {
    // Keep the last saved conversation when opening an unsent draft.
    if (taskId) {
      try { window.localStorage.setItem(LAST_CONVERSATION_KEY, taskId); } catch { /* Storage can be disabled. */ }
    }
    if (taskId && recordVisit) navigation.record(taskId);
    changeSelection(selectedTaskIdRef.current, taskId, () => {
      composerContext.current++;
      selectedTaskIdRef.current = taskId;
      timelineEpoch.current.invalidate();
      approvalEpoch.current.invalidate();
      quotaEpoch.current.invalidate();
      const cached = taskId ? conversationCache.current.get(taskId) : undefined;
      setTimeline(cached?.items ?? []);
      if (cached) setLoadedTaskId(taskId);
      setPendingApprovals([]);
      setTimelineLoading(taskId !== null && !cached);
      setTimelineError(null);
      setSelectedTaskId(taskId);
    });
  }, [navigation.record]);

  const loadConversation = useCallback(async (taskId: TaskId, includeApprovals = true) => {
    const isCurrentTimeline = timelineEpoch.current.begin();
    const isCurrentApproval = includeApprovals ? approvalEpoch.current.begin() : () => false;
    let stream = timelineStreams.current.get(taskId);
    if (!stream) { stream = new StreamingTimeline(); timelineStreams.current.set(taskId, stream); }
    const read = client.timelineSnapshot ? stream.beginRead() : null;
    const timelineRequest = client.timelineSnapshot ? client.timelineSnapshot(taskId) : client.timeline(taskId).then(items => ({ items, lastSequence: 0 }));
    // Cursor and items come from one snapshot. Replay only events newer than it.
    await Promise.all([
      timelineRequest.then(result => {
        // Render messages immediately; mounted file cards load their own artifacts.
        if (!isCurrentTimeline()) return;
        const projected = read ? stream.replace(read, result) : { items: result.items, dirty: false };
        conversationCache.current.set(taskId, projected);
        setTimeline(projected.items);
        traceStartup("timeline_ready");
        setTimelineError(null);
      }).catch(() => {
        if (isCurrentTimeline()) setTimelineError("对话加载失败，请重试");
      }).finally(() => {
        if (read) stream.endRead(read);
        if (isCurrentTimeline()) { setLoadedTaskId(taskId); setTimelineLoading(false); }
      }),
      includeApprovals ? client.pendingApprovals(taskId).then(approvals => {
        if (isCurrentApproval()) setPendingApprovals(approvals);
      }).catch(() => { /* Keep the last known approvals on transient failure. */ }) : Promise.resolve(),
    ]);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const next = await client.bootstrap();
      traceStartup("bootstrap_ready");
      const priorTasks = previousTasks.current;
      previousTasks.current = next.tasks;
      setSnapshot({ ...next, tasks: next.tasks.map(task => ({ ...task,
        streamVersion: Math.max(task.streamVersion, streamVersions.current.get(task.taskId) ?? 0) })) });
      const visibleIds = new Set(next.tasks.map(task => task.taskId));
      for (const id of conversationCache.current.keys()) {
        if (!visibleIds.has(id)) { conversationCache.current.delete(id); timelineStreams.current.delete(id); streamVersions.current.delete(id); }
      }
      const currentId = selectedTaskIdRef.current;
      if (currentId && !next.tasks.some(task => task.taskId === currentId)) {
        const deleted = priorTasks.find(task => task.taskId === currentId);
        const available = [...next.tasks].reverse();
        const replacement = available.find(task => task.projectId === deleted?.projectId) ?? available[0];
        if (!replacement && deleted) { setDraftProjectId(deleted.projectId); setDraftExecutor(deleted.preferredExecutor); }
        selectTask(replacement?.taskId ?? null);
        setActiveView("conversation"); setSubagentItemId(null);
      }
      setStartupError(false);
      if (!initialSelectionHandled.current) {
        initialSelectionHandled.current = true;
        const lastViewed = readPreference<string>(LAST_CONVERSATION_KEY, "");
        const restoredTask = next.tasks.find(task => task.taskId === lastViewed);
        selectTask(restoredTask?.taskId ?? next.tasks[0]?.taskId ?? null);
      }
    } catch {
      setStartupError(true);
      setError("对话暂时无法更新，请稍后重试。");
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Quota snapshots change slowly; a 30s poll plus per-turn refreshes keep
  // the ring and panel honest without hammering the drivers.
  useEffect(() => {
    let active = true;
    setQuotaViews([]);
    setQuotaTaskId(selectedTaskId);
    setQuotaRefreshProgress(null);
    const reader = createQuotaRefresh(client, selectedTaskId, view => {
      if (selectedTaskIdRef.current !== selectedTaskId) return;
      setQuotaViews(previous => previous.some(old => old.installationId === view.installationId)
        ? previous.map(old => old.installationId === view.installationId ? view : old)
        : [...previous, view]);
    }, setQuotaRefreshProgress);
    refreshQuotasNow.current = () => { void reader.refreshNow().then(() => { if (active) void refresh(); }); };
    refreshSessionUsage.current = (id, provider) => { void reader.refreshInstallation(id, provider); };
    const load = () => { void reader.refresh().catch(() => {}); };
    refreshQuotas.current = load;
    let unlisten: (() => void) | undefined;
    void client.onQuotaProviderReady?.(load).then(stop => {
      if (active) { unlisten = stop; load(); } else stop();
    }).catch(() => {});
    load();
    const timer = setInterval(load, 30_000);
    return () => {
      active = false;
      reader.dispose();
      unlisten?.();
      refreshQuotas.current = () => {};
      refreshQuotasNow.current = () => {};
      refreshSessionUsage.current = () => {};
      quotaEpoch.current.invalidate();
      clearInterval(timer);
    };
  }, [selectedTaskId]);

  useEffect(() => {
    selectedTaskIdRef.current = selectedTaskId;
    if (!selectedTaskId) return;
    const cached = conversationCache.current.get(selectedTaskId);
    if (cached) {
      setTimeline(cached.items); setLoadedTaskId(selectedTaskId); setTimelineLoading(false); setTimelineError(null);
    } else { setTimeline([]); setTimelineLoading(true); }
    if (!cached || cached.dirty) void loadConversation(selectedTaskId);
    else {
      const isCurrent = approvalEpoch.current.begin();
      void client.pendingApprovals(selectedTaskId).then(items => { if (isCurrent()) setPendingApprovals(items); }).catch(() => {});
    }
    return () => {
      timelineEpoch.current.invalidate();
      approvalEpoch.current.invalidate();
    };
  }, [selectedTaskId, loadConversation]);

  const conversationRun = useMemo(() => snapshot?.tasks.find(task => task.taskId === selectedTaskId)?.activePrimaryRun, [snapshot?.tasks, selectedTaskId]);
  const conversationStatus = `${conversationRun?.turnId ?? ""}:${conversationRun?.state ?? ""}:${pendingSubmission?.taskId === selectedTaskId ? pendingSubmission.providerId : ""}`;

  const subscriptionReady = snapshot !== null;
  useEffect(() => {
    if (!subscriptionReady || !snapshot) return;
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
    let processing = Promise.resolve();
    let projectionTimer: ReturnType<typeof setTimeout> | undefined;
    let projectionBusy = false, projectionDirty = false, globalDirty = false;
    let frameTimer: ReturnType<typeof setTimeout> | undefined;
    const pendingStreams = new Set<TaskId>();
    const requestFrame = () => {
      if (frameTimer || disposed) return;
      frameTimer = setTimeout(() => {
        frameTimer = undefined;
        if (disposed) return;
        for (const taskId of pendingStreams) {
          const stream = timelineStreams.current.get(taskId);
          if (!stream) continue;
          const result = stream.flush();
          const cached = conversationCache.current.get(taskId);
          if (cached) {
            if (result.items) cached.items = result.items;
            cached.dirty ||= result.dirty;
          }
          if (taskId === selectedTaskIdRef.current) {
            if (cached) setTimeline(cached.items);
            if (result.dirty) requestProjection(false);
          }
        }
        pendingStreams.clear();
      }, 16);
    };
    const requestProjection = (global = true) => {
      projectionDirty = true; globalDirty ||= global;
      if (projectionBusy || projectionTimer || disposed) return;
      projectionTimer = setTimeout(() => {
        const refreshGlobal = globalDirty;
        projectionTimer = undefined; projectionBusy = true; projectionDirty = false; globalDirty = false;
        void refreshFromProjection(refreshGlobal).catch(() => { if (!disposed) setError("对话更新失败，正在等待下一次同步"); })
          .finally(() => { projectionBusy = false; if (projectionDirty && !disposed) requestProjection(false); });
      }, 40);
    };
    const cursor = new OrderedEventCursor(snapshot.lastSequence);
    const refreshFromProjection = async (global: boolean) => {
      if (global) await refresh();
      if (disposed) return;
      const taskId = selectedTaskIdRef.current;
      if (taskId) {
        if (global) refreshQuotas.current();
        await loadConversation(taskId, global);
      }
    };
    const applyEvent = (event: import("@dogcodes/protocol").EventEnvelope) => {
      if (event.eventType === "task.deleted") window.dispatchEvent(new Event("dogcodes:task-deleted"));
      const taskId = event.stream.streamType === "task" ? event.stream.streamId as TaskId : null;
      if (taskId && event.eventType === "turn.queued" && event.causationId) {
        setPendingSubmission(current => current?.operationId === event.causationId
          ? { ...current, acceptedSequence: event.sequence, taskId } : current);
      }
      const failure = event.payload as { failureKind?: string };
      if (event.eventType === "turn.failed" && taskId === selectedTaskIdRef.current && failure?.failureKind === "model_unavailable") {
        refreshModelCatalog.current();
      }
      if (event.eventType === "turn.failed" && taskId && taskId === selectedTaskIdRef.current && ["selection_rejected", "selection_unverifiable"].includes(failure?.failureKind ?? "")) {
        const submitted = submittedSelections.current.get(taskId);
        setNewSessionModel(current => current === submitted ? null : current);
        submittedSelections.current.delete(taskId);
      }
      const stream = taskId ? timelineStreams.current.get(taskId) : undefined;
      const applied = client.timelineSnapshot && stream?.receive(event);
      const cached = taskId ? conversationCache.current.get(taskId) : undefined;
      if (taskId) streamVersions.current.set(taskId, Math.max(streamVersions.current.get(taskId) ?? 0, event.streamVersion));
      if (["usage.request_recorded", "turn.first_output", "turn.usage_recorded"].includes(event.eventType)) {
        if (cached) cached.dirty = true;
        return; // The terminal projection loads frozen statistics; no per-request global refresh.
      }
      if (client.timelineSnapshot && isTimelineDelta(event)) {
        if (!applied && cached) cached.dirty = true;
        if (taskId && stream) { pendingStreams.add(taskId); requestFrame(); }
        const data = (event.payload as { data?: { type?: string; status?: string } })?.data;
        // Live thought text is deliberately kept outside the canonical event
        // store. Refresh this one timeline, never global state or approvals.
        if (taskId === selectedTaskIdRef.current && (!applied || (data?.type === "thought" && data.status === "running"))) requestProjection(false);
      } else {
        if (cached) cached.dirty = true;
        if (!client.timelineSnapshot) for (const entry of conversationCache.current.values()) entry.dirty = true;
        requestProjection();
      }
    };
    // A retryable stream error means the backend pump has stopped; the only
    // way to keep receiving ordered events is to subscribe again from the
    // last applied sequence. Duplicates are dropped by the cursor.
    const scheduleResubscribe = (delay: number) => {
      if (disposed) return;
      reconnectTimer = setTimeout(() => {
        if (disposed) return;
        unsubscribe?.();
        unsubscribe = undefined;
        void client.subscribe(cursor.lastSequence, handleMessage).then((stop) => {
          if (disposed) {
            stop();
            return;
          }
          unsubscribe = stop;
          setError(null);
        }).catch(() => scheduleResubscribe(Math.min(delay * 2, 8_000)));
      }, delay);
    };
    const handleMessage = (message: EventStreamMessage) => {
      processing = processing.then(async () => {
        if (disposed) return;
        if (message.type === "error") {
          setError("连接暂时中断，正在尝试恢复。");
          if (message.data.retryable) scheduleResubscribe(500);
          return;
        }
        if (message.type !== "event") return;
        const result = cursor.accept(message.data);
        if (result === "duplicate") return;
        if (result === "gap") {
          const missing = await client.syncEvents(cursor.lastSequence);
          for (const event of missing) {
            const recovered = cursor.accept(event);
            if (recovered === "gap") throw new Error("事件序列无法补齐");
            if (recovered === "applied") applyEvent(event);
          }
          const retried = cursor.accept(message.data);
          if (retried === "gap") throw new Error("事件序列无法补齐");
          if (retried === "applied") applyEvent(message.data);
        } else applyEvent(message.data);
      }).catch((cause) => {
        if (!disposed) setError("对话暂时无法同步，请稍后重试。");
      });
    };

    void client.subscribe(cursor.lastSequence, handleMessage).then((stop) => {
      if (disposed) stop();
      else unsubscribe = stop;
    }).catch((cause) => {
      if (!disposed) setError("暂时无法接收对话更新，请稍后重新打开应用。");
    });
    return () => {
      disposed = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      if (projectionTimer) clearTimeout(projectionTimer);
      if (frameTimer) clearTimeout(frameTimer);
      unsubscribe?.();
    };
    // Re-subscribe only when bootstrap changes from unavailable to available.
    // Projection refreshes must not reset the sequence cursor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [subscriptionReady, refresh, loadConversation]);

  const selectedTask = useMemo(
    () => {
      // This draft summary is only a view model; its ID must never reach task APIs.
      if (!selectedTaskId && snapshot) {
        const project = snapshot.projects.find(project => project.projectId === draftProjectId)
          ?? snapshot.projects.find(project => project.displayName === "快速对话") ?? snapshot.projects[0];
        if (!project) return null;
        return { taskId: "tsk_draft" as TaskId, projectId: project.projectId, title: "新对话",
          preferredExecutor: currentAgentExecutor((draftExecutor && !agentPreferences.disabled.includes(draftExecutor.providerId) ? draftExecutor : defaultExecutor(snapshot, agentPreferences)), snapshot.installations), activePrimaryRun: null, streamVersion: 0 };
      }
      const task = snapshot?.tasks.find((item) => item.taskId === selectedTaskId);
      if (!task) return null;
      const preferredExecutor = nextExecutors[task.taskId] ?? task.preferredExecutor;
      // Reloaded tasks and Fast Refresh state can retain a replaced installation.
      // Resolve all composer controls together, while preserving an in-flight run.
      return { ...task, preferredExecutor: task.activePrimaryRun ? preferredExecutor
        : currentAgentExecutor(preferredExecutor, snapshot!.installations) };
    },
    [selectedTaskId, snapshot, nextExecutors, draftProjectId, draftExecutor, agentPreferences],
  );
  const pendingExecutors = useMemo(() => snapshot
    ? Object.fromEntries(Object.entries(nextExecutors).map(([taskId, executor]) => [taskId, currentAgentExecutor(executor, snapshot.installations)]))
    : {}, [nextExecutors, snapshot]);
  const executorOptions = useMemo(() => {
    const options: ExecutorOption[] = [];
    const visibleReadyInstallations = enabledAgents(readyAgentInstallations(snapshot?.installations ?? []), item => item.providerId, agentPreferences);
    for (const installation of visibleReadyInstallations) {
      const executor = {
        providerId: installation.providerId,
        installationId: installation.installationId,
      };
      options.push({
        key: executorKey(executor),
        executor,
        quota: quotaViews.find((view) =>
          view.providerId === installation.providerId
          && view.installationId === installation.installationId)?.quota ?? null,
      });
    }
    return options;
  }, [snapshot?.installations, quotaViews, agentPreferences]);


  const selectedMapping = snapshot?.sessionMaps.find(mapping => mapping.taskId === selectedTask?.taskId
    && mapping.executor.providerId === selectedTask?.preferredExecutor.providerId
    && mapping.executor.installationId === selectedTask?.preferredExecutor.installationId);
  const modelReadyIds = useModelReadyExecutors(client);
  const modelVersion = snapshot?.installations.find(i=>i.installationId===selectedTask?.preferredExecutor.installationId)?.version ?? null;
  const selectedModelReady = !client.modelReadyExecutors || modelReadyIds.includes(`${selectedTask?.preferredExecutor.providerId}:${selectedTask?.preferredExecutor.installationId}`);
  const modelSelectionKey = `${selectedTask?.taskId}:${selectedTask?.preferredExecutor.providerId}:${selectedTask?.preferredExecutor.installationId}`;
  const pendingModel = newSessionModel?.key === modelSelectionKey ? newSessionModel : null;
  const activeModel = pendingModel ? pendingModel.model : selectedMapping?.model ?? null;
  const activeThinking = pendingModel ? pendingModel.thinking : selectedMapping?.thinking ?? null;
  const modelCatalog = useModelCatalog(modelCache, selectedTask?.preferredExecutor, modelVersion,
    selectedTaskId ?? undefined, activeModel, selectedModelReady, !!selectedTask?.activePrimaryRun);
  const { catalog: nativeModels, loading: modelLoading, error: modelError } = modelCatalog;
  useEffect(() => { refreshModelCatalog.current = modelCatalog.refresh; }, [modelCatalog.refresh]);
  const permissionKey = selectedTask ? `${selectedTask.taskId}:${selectedTask.preferredExecutor.providerId}:${selectedTask.preferredExecutor.installationId}` : "";
  const permissionPicker = selectedTask ? <PermissionPicker key={permissionKey} version={modelVersion} client={client} executor={selectedTask.preferredExecutor} taskId={selectedTaskId ?? undefined}
    run={selectedTask.activePrimaryRun} disabled={sending} onChange={mode=>setPermissionSelection({key:permissionKey,mode})} /> : null;
  const modelPicker = selectedTask && selectedTask.preferredExecutor.providerId !== "fake" ? <ModelPicker key={modelSelectionKey}
    catalog={nativeModels} model={activeModel} thinking={activeThinking} loading={modelLoading} error={modelError}
    disabled={sending}
    onChange={(model, thinking) => setNewSessionModel({ key: modelSelectionKey, model, thinking })}
    onRetry={modelCatalog.refresh} onOpen={modelCatalog.check} refreshDisabled={!!selectedTask.activePrimaryRun} /> : null;

  const selectedQuota = useMemo(() => {
    if (!selectedTask || quotaTaskId !== selectedTask.taskId) return null;
    const executor = selectedTask.preferredExecutor;
    const quota = (
      quotaViews.find(
        (view) =>
          view.providerId === executor.providerId
            && view.installationId === executor.installationId,
      )?.quota ?? null
    );
    return executor.providerId === "antigravity" ? antigravityQuotaForModel(quota, activeModel) : quota;
  }, [quotaViews, quotaTaskId, selectedTask, activeModel]);
  useEffect(() => {
    const executor = selectedTask?.preferredExecutor;
    if (!selectedTask?.activePrimaryRun || !executor?.installationId) return;
    const timer = setInterval(() => refreshSessionUsage.current(executor.installationId!, executor.providerId), 5_000);
    return () => clearInterval(timer);
  }, [selectedTask?.taskId, !!selectedTask?.activePrimaryRun, selectedTask?.preferredExecutor.installationId, selectedTask?.preferredExecutor.providerId]);
  // Acceptance assigns the real task ID before bootstrap can select it. Keep
  // the prompt attached to its draft throughout that handoff, including slow
  // refreshes, without carrying it into a later draft with a different scope.
  const selectedSubmission = pendingSubmission && (pendingSubmission.taskId === selectedTask?.taskId
    || (!selectedTaskId && pendingSubmission.scope === newDraftScope.current)) ? pendingSubmission : null;
  const submittedMessage = selectedSubmission ? timeline.find(item => item.taskId === selectedSubmission.taskId
    && item.kind === "user_message" && item.sequence === selectedSubmission.acceptedSequence) : undefined;
  // Keep the local prompt until the canonical timeline catches up, even if the
  // submit acknowledgement arrives before the subscription's projection refresh.
  const displayedTimeline = useMemo(() => selectedSubmission && !submittedMessage
    ? [...timeline, { ...selectedSubmission.message, taskId: selectedSubmission.taskId }]
    : timeline, [timeline, selectedSubmission, submittedMessage]);
  useEffect(() => {
    if (!sending && submittedMessage) setPendingSubmission(null);
  }, [sending, submittedMessage]);
  const latestUserMessageId = useMemo(() => {
    for (let index = displayedTimeline.length - 1; index >= 0; index -= 1) {
      if (displayedTimeline[index].kind === "user_message") return displayedTimeline[index].itemId;
    }
    return undefined;
  }, [displayedTimeline]);
  const conversationReady = displayedTimeline.length > 0 && (Boolean(selectedSubmission) || loadedTaskId === selectedTaskId);
  const conversationLoader = useLoadingVisibility(!conversationReady && !!selectedTaskId && (timelineLoading || loadedTaskId !== selectedTaskId));
  // A loader that has appeared stays briefly, so it never flashes just before the conversation.
  const showConversation = conversationReady && !conversationLoader.lingering;
  const pendingAgent: PendingAgent | undefined = conversationRun
    ? (["queued", "starting", "running"].includes(conversationRun.state)
      ? { providerId: conversationRun.executor.providerId, turnId: conversationRun.turnId } : undefined)
    : selectedSubmission
      ? { providerId: selectedSubmission.providerId, turnId: submittedMessage?.turnId ?? undefined }
      : undefined;
  const renderResponseAgent = (providerId: string) => <span role="img" aria-label={`${providerLabel(providerId)} 回答`} data-tooltip={providerLabel(providerId)}><AgentIcon providerId={providerId} size="small" /></span>;
  const selectedTurnQuotaExhausted = useMemo(() => {
    const providerId = selectedTask?.preferredExecutor.providerId;
    if (!providerId) return false;
    for (let index = timeline.length - 1; index >= 0; index -= 1) {
      const item = timeline[index];
      if (item.providerId !== providerId) continue;
      if (item.kind === "quota.exhausted") return true;
      if (item.kind === "assistant_message") return false;
    }
    return false;
  }, [selectedTask?.preferredExecutor.providerId, timeline]);
  const quotaSendingBlocked = quotaIsExhausted(selectedQuota);
  const [quotaClock, setQuotaClock] = useState(0);
  useEffect(() => {
    // Re-evaluate the gate at reset/TTL boundaries even if native polling hangs.
    const now = Date.now();
    const deadlines = quotaViews.flatMap(view => view.quota ? [
      view.quota.staleAfterUnixMs,
      ...view.quota.windows.flatMap(window => window.resetsAtUnix == null ? [] : [window.resetsAtUnix * 1000]),
    ] : []).filter(deadline => Number.isFinite(deadline) && deadline > now);
    if (!deadlines.length) return;
    const timer = setTimeout(() => setQuotaClock(value => value + 1), Math.min(Math.min(...deadlines) - now + 1, 2_147_483_647));
    return () => clearTimeout(timer);
  }, [quotaViews, quotaClock]);
  const selectedInstallation = snapshot?.installations.find(
    (installation) => installation.installationId === selectedTask?.preferredExecutor.installationId,
  );

  const selectedProject = snapshot?.projects.find((project) => project.projectId === selectedTask?.projectId) ?? null;
  const preferredExecutor = selectedTask?.preferredExecutor ?? (snapshot ? defaultExecutor(snapshot, agentPreferences) : null);
  const readyAgentCount = executorOptions.length;


  const createConversation = useCallback((projectId?: ProjectId) => {
    if (!snapshot || sending || switchBusy.current) return;
    newDraftScope.current = `draft:${ulid()}`;
    composerContext.current++;
    setDraftProjectId(projectId ?? snapshot.projects.find(project => project.displayName === "快速对话")?.projectId ?? null);
    setDraftExecutor(selectedTask?.preferredExecutor ?? defaultExecutor(snapshot, agentPreferences));
    selectTask(null);
    setSubagentItemId(null);
    setError(null);
    if (!projectId) { setDraft(""); clearAttachments(); }
    setActiveView("conversation");
  }, [snapshot, sending, selectedTask?.preferredExecutor, agentPreferences, selectTask, clearAttachments]);
  const createProjectConversation = useCallback((project: ProjectSummary) => {
    if (sending || creatingTask || switchBusy.current) return;
    createConversation(project.projectId);
    setDraft("");
    clearAttachments();
  }, [sending, creatingTask, createConversation, clearAttachments]);

  const openProjectFolder = useCallback(() => {
    if (creatingTask || switchBusy.current) return;
    setProjectDialogOpen(true);
  }, [creatingTask]);
  const selectHistoryTask = useCallback((taskId: TaskId) => { selectTask(taskId); setActiveView("conversation"); }, [selectTask]);
  const activeReviewTasks = useMemo(() => snapshot?.companion.workspaces.filter(row => row.secondaryAgentCount > 0).map(row => row.taskId) ?? [], [snapshot?.companion.workspaces]);
  const deleteHistoryTask = useCallback(async (task: TaskSummary) => {
    const latest = (await client.bootstrap()).tasks.find(candidate => candidate.taskId === task.taskId);
    await client.deleteTask!(latestTask(latest ?? task));
    await refresh();
  }, [client, latestTask, refresh]);
  const renameHistoryTask = useCallback(async (task: TaskSummary, title: string) => {
    const latest = (await client.bootstrap()).tasks.find(candidate => candidate.taskId === task.taskId);
    if (!latest) throw new Error("会话不存在");
    await client.renameTask!(latestTask(latest), title);
    await refresh();
  }, [client, latestTask, refresh]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!selectedTask || submissionPending || (!draft.trim() && !attachments.items.length) || attachments.pending > 0 || (activeModel && (modelLoading || modelError)) || sending || creatingTask || switchBusy.current || selectedTask.activePrimaryRun || quotaSendingBlocked || agentPreferences.disabled.includes(selectedTask.preferredExecutor.providerId) || (agentPreferences.disabled.length > 0 && !executorOptions.length)) return;
    const prompt = draft.trim(), attachmentIds = attachments.items.map(item => item.id);
    const originContext = composerContext.current;
    const scope = selectedTaskId ? `task:${selectedTaskId}` : newDraftScope.current;
    switchBusy.current = true;
    setSending(true);
    const optimisticId = `pending-${ulid()}`;
    setPendingSubmission({ scope, taskId: selectedTask.taskId, providerId: selectedTask.preferredExecutor.providerId,
      message: { itemId: optimisticId, taskId: selectedTask.taskId, turnId: null,
        kind: "user_message", text: prompt, providerId: null, sequence: 0, occurredAt: new Date().toISOString(),
        attachments: attachments.items.map(item => ({ name: item.name, path: item.path ?? "", mimeType: "application/octet-stream" })) } });
    setDraft("");
    const clearedRevision = draftRevision.current;
    setError(null);
    // Set once the service holds the original; recovery then owns its content.
    let durable = false;
    let activeContext = originContext;
    const operationId = `cmd_${ulid()}`;
    try {
      const prepared = await attachments.prepare();
      setPendingSubmission(current => current?.message.itemId === optimisticId
        ? { ...current, operationId, message: { ...current.message, attachments: prepared } } : current);
      let result: SubmissionResolution;
      try {
        result = await submissionOperations.start(client, { operationId, scope, taskId: selectedTaskId, projectId: selectedTask.projectId,
          target: selectedTask.preferredExecutor, input: { prompt, model: activeModel, thinking: activeThinking,
            permissionMode: permissionSelection?.key === permissionKey ? permissionSelection.mode : null, attachments: prepared } });
      } catch (cause) {
        if (cause instanceof PendingSubmissionError) {
          durable = true;
          attachments.consume(attachmentIds);
          submittedSelections.current.set(cause.intent.task.taskId, pendingModel);
        } else if (cause instanceof UnconfirmedSubmissionError) {
          // Outcome unknown: keep the original identity for verification and never
          // refill the composer, or a resend under a new id could run twice.
          durable = true;
          attachments.consume(attachmentIds);
        }
        throw cause;
      }
      durable = true;
      // The service's original now owns only these attachments. Later selections stay.
      attachments.consume(attachmentIds);
      submittedSelections.current.set(result.taskId, pendingModel);
      setPendingSubmission(current => current?.operationId === operationId
        ? result.status === "submitted" ? { ...current, acceptedSequence: result.sequence, taskId: result.taskId } : null : current);
      await refresh();
      if (!selectedTaskId && result.status === "submitted" && composerContext.current === originContext && draftRevision.current === clearedRevision) {
        selectTask(result.taskId);
        activeContext = composerContext.current;
      }
      if (composerContext.current === activeContext && draftRevision.current === clearedRevision) setNextExecutors(previous => {
        const next = { ...previous }; delete next[selectedTask.taskId]; return next;
      });
    } catch (cause) {
      setPendingSubmission(current => current?.message.itemId === optimisticId ? null : current);
      // Only failures before the service stored the original return to the input.
      // Once stored, recovery owns the original content and prevents a fresh resend.
      if (!durable && composerContext.current === originContext && draftRevision.current === clearedRevision) setDraft(prompt);
      if (composerContext.current === activeContext) setError(cause instanceof UnconfirmedSubmissionError
        ? "发送结果未知，原请求和发送标识已保留，请在下方核实后再处理。"
        : userFacingError(cause, durable ? "发送结果尚未确认，原内容已保存，请核实原发送。" : "消息未能发送，内容已保留，请重试。"));
    } finally {
      setSending(false);
      switchBusy.current = false;
    }
  };

  const answerApproval = async (approval: PendingApproval, optionId: string, answer?: import("@dogcodes/protocol").QuestionAnswer) => {
    if (!selectedTask || approvalBusy.current) return;
    approvalBusy.current = true;
    setApproving(approval.approvalId);
    setApprovalError(null);
    setError(null);
    try {
      await approvalOperations.submit(client, latestTask(selectedTask), approval, optionId, answer);
      if (selectedTaskIdRef.current === selectedTask.taskId) {
        approvalEpoch.current.invalidate();
        setPendingApprovals(current => current.filter(item => item.approvalId !== approval.approvalId
          || item.turnId !== approval.turnId || item.requestedAtSequence !== approval.requestedAtSequence));
        void loadConversation(selectedTask.taskId);
      }
    } catch (cause) {
      if (selectedTaskIdRef.current === selectedTask.taskId) setApprovalError({ taskId: selectedTask.taskId, approval,
        message: userFacingError(cause, "审批结果暂时无法确认，请先核实原请求。") });
    } finally {
      approvalBusy.current = false;
      setApproving(null);
    }
  };

  const approvalPanel = pendingApprovals.length > 0 ? <div className="approval-dock" aria-label="待审批操作">
    {pendingApprovals.length > 1 && <p className="approval-count">{pendingApprovals.length} 项请求等待你处理</p>}
    {pendingApprovals.map(approval => <ApprovalCard key={`${approval.turnId}:${approval.requestedAtSequence}:${approval.approvalId}`} approval={approval}
      busy={approving === approval.approvalId} disabled={(approving !== null && approving !== approval.approvalId)
        || !Array.isArray(pendingApprovalResponses) || pendingApprovalResponses.some(record => record.accepted
          && record.query.approvalId === approval.approvalId && record.query.turnId === approval.turnId && record.query.requestedAtSequence === approval.requestedAtSequence)}
      onRespond={(optionId, answer) => void answerApproval(approval, optionId, answer)} />)}
    {selectedTask?.activePrimaryRun?.executor.providerId === "cursor" && <div className="approval-permissions">{permissionPicker}</div>}
    {error && <p className="error" role="alert">{error}</p>}
  </div> : null;

  const cancelPending = !Array.isArray(pendingCancellations) || pendingCancellations.some(record => record.task.activePrimaryRun?.runId === selectedTask?.activePrimaryRun?.runId);
  const cancelActiveTurn = async () => {
    if (!selectedTask?.activePrimaryRun || cancelling || cancelGuard.current || cancelPending) return;
    cancelGuard.current = true;
    setCancelling(true);
    setError(null);
    try {
      await cancelOperations.start(client, latestTask(selectedTask));
    } catch (cause) {
      if (selectedTaskIdRef.current === selectedTask.taskId) setError(userFacingError(cause, "停止结果尚未确认，请核实原请求。"));
    } finally {
      cancelGuard.current = false;
      setCancelling(false);
    }
  };

  const switchAgent = (targetExecutor: ExecutorRef) => {
    if (!selectedTask || switchBusy.current || sending) return;
    setError(null);
    if (!selectedTaskId) setDraftExecutor(targetExecutor);
    else setNextExecutors(previous => ({ ...previous, [selectedTask.taskId]: targetExecutor }));
  };

  const toggleSidebar = () => {
    const next = !sidebarCollapsed;
    setSidebarCollapsed(next);
    setPreferencesOpen(false);
    try { localStorage.setItem("dogcodes.sidebar.collapsed", String(next)); } catch { /* Optional preference. */ }
  };
  const navigateConversation = (direction: -1 | 1) => {
    const taskId = navigation.move(direction);
    if (taskId) { selectTask(taskId, false); setActiveView("conversation"); }
  };
  const windowControls = <SidebarWindowControls collapsed={sidebarCollapsed} onToggle={toggleSidebar} canBack={navigation.canBack} canForward={navigation.canForward} onBack={() => navigateConversation(-1)} onForward={() => navigateConversation(1)} />;

  if (welcomeOpen) return <AgentWelcome client={client}
    onDetected={(provider, detected) => setSnapshot(current => current ? { ...current,
      installations: [...current.installations.filter(i => i.providerId !== provider), ...detected.installations.filter(i => i.providerId === provider)],
    } : detected)} onContinue={closeWelcome} renderIcon={id => <AgentIcon providerId={id} />} />;
  if (!snapshot) return startupError ? <StartupRecovery status="unavailable" onRetry={() => { setStartupError(false); setError(null); void refresh(); }} /> : <StartupShell />;
  if (!selectedTask) {
    return <StartupRecovery status="unavailable" onRetry={() => { selectTask(null); void refresh(); }} />;
  }

  return (
    <I18nProvider initialLanguage={uiLanguage}>
      <main className={`app-shell${sidebarCollapsed ? " sidebar-collapsed" : ""}${activeView === "conversation" && reviewOpen && !reviewWindow.detached ? " review-expanded" : ""}`} style={{ "--sidebar-width": `${sidebarWidth.width}px`, "--review-width": `${reviewWidth.width}px` } as React.CSSProperties}>
        <aside id="app-sidebar" className="sidebar" hidden={sidebarCollapsed}>
        <header className="sidebar-titlebar" data-tauri-drag-region="deep">
          {!sidebarCollapsed && windowControls}
        </header>
        <nav className="primary-nav" aria-label={t("nav.mainNav", undefined, resolvedLang)}>
          <button ref={newConversationButton} aria-keyshortcuts={`${shortcutKey}+N`} className="primary-action" type="button" disabled={sending || creatingTask || snapshot.projects.length === 0} onClick={() => void createConversation()}>
            <SidebarNavIcon kind="conversation" /><b>{creatingTask ? t("nav.creating", undefined, resolvedLang) : t("nav.newConversation", undefined, resolvedLang)}</b><kbd>{shortcutModifier} N</kbd>
          </button>
          <button ref={quotaButton} aria-keyshortcuts={`${shortcutKey}+U`} className={activeView === "quota" ? "active" : ""} type="button" onClick={() => setActiveView("quota")}><SidebarNavIcon kind="quota" /><b>{t("nav.agentManagement", undefined, resolvedLang)}</b><kbd>{shortcutModifier} U</kbd></button>
          <button className={activeView === "remote" ? "active" : ""} type="button" aria-current={activeView === "remote" ? "page" : undefined} onClick={() => setActiveView("remote")}><SidebarNavIcon kind="phone" /><b>{t("remoteDevices.title", undefined, resolvedLang)}</b></button>
        </nav>
        <div className="sidebar-divider" />
        <MemoConversationHistory onRevealProject={revealLocalProject} onCreateProjectConversation={createProjectConversation} createConversationDisabled={sending || creatingTask}
          onCreateProject={openProjectFolder} onEditProject={client.updateProject ? setEditingProject : undefined} projects={snapshot.projects} tasks={snapshot.tasks} selectedTaskId={selectedTaskId}
          activeReviewTasks={activeReviewTasks} selectionVisible={activeView === "conversation"} onDelete={client.deleteTask ? deleteHistoryTask : undefined}
          onRename={client.renameTask ? renameHistoryTask : undefined} onSelect={selectHistoryTask} />
        <div className="sidebar-preferences" ref={preferencesRef}>
          <button
            className="preferences-trigger"
            data-tooltip={labels.title}
            type="button"
            aria-label={labels.open}
            aria-keyshortcuts="Control+0 Meta+0"
            aria-expanded={preferencesOpen}
            onClick={() => {
              setPreferencesOpen((open) => !open);
              setPreferencesSection(null);
            }}
          >
            <img className="brand-mark" src={appLogo} alt="" aria-hidden="true" draggable={false} />
            <span className="brand-name">dog.codes</span>
            <svg className="preferences-gear" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" /><circle cx="12" cy="12" r="3" /></svg>
          </button>
          <PreferencesMenu
            anchorRef={preferencesRef}
            popoverRef={preferencesPopoverRef}
            open={preferencesOpen}
            section={preferencesSection}
            language={uiLanguage}
            theme={uiTheme}
            scale={uiScale}
            onToggle={(next) => setPreferencesSection((current) => current === next ? null : next)}
            onLanguage={(next) => { setUiLanguage(next); setPreferencesOpen(false); setPreferencesSection(null); }}
            onTheme={(next) => { setUiTheme(next); setPreferencesOpen(false); setPreferencesSection(null); }}
            onScale={action => setUiScale(current => adjustUiScale(current, action))}
            onRestartCore={restartBackground}
          />
        </div>
      </aside>

      {!sidebarCollapsed && <SidebarResizeHandle width={sidebarWidth.width} max={sidebarWidth.max} defaultWidth={sidebarWidth.defaultWidth} onResize={sidebarWidth.setWidth} />}
      <div className="conversation-column global-content">
        <header className="task-header" data-tauri-drag-region="deep">
          {sidebarCollapsed && <div className="collapsed-sidebar-controls">{windowControls}</div>}
          {activeView === "quota"
            ? <div className="task-heading"><h1>{t("agentManagement.title", undefined, resolvedLang)}</h1></div>
            : activeView === "remote"
            ? <div className="task-heading"><h1>{t("remoteDevices.title", undefined, resolvedLang)}</h1></div>
            : <div className="task-heading"><h1>{selectedTask.title}</h1><span>{selectedProject ? projectLabel(selectedProject.displayName) : t("header.temporaryWorkspace", undefined, resolvedLang)}</span></div>}
          <div className="task-tools">
            {activeView === "conversation" && <>
            <button className="panel-toggle" type="button" aria-label={t("header.toggleTerminal", undefined, resolvedLang)} aria-expanded={terminalOpen} data-tooltip={terminalOpen ? t("header.collapseTerminal", undefined, resolvedLang) : t("header.toggleTerminal", undefined, resolvedLang)} onClick={() => { setTerminalLoaded(true); setTerminalOpen(open => !open); }}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><rect x="3" y="3" width="18" height="18" rx="3" /><path d="m7 8 3 3-3 3m6 1h4" /></svg>
            </button>
            <button className="panel-toggle" type="button" aria-label={t("header.toggleReview", undefined, resolvedLang)} disabled={!selectedTaskId} aria-expanded={reviewOpen && !reviewWindow.detached} data-tooltip={reviewWindow.detached ? t("header.detachedReview", undefined, resolvedLang) : t("header.toggleReview", undefined, resolvedLang)} onClick={() => { if (reviewWindow.detached) void reviewWindow.dock(); else setReviewOpen(open => !open); }}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><rect x="3" y="3" width="18" height="18" rx="3" /><path d="M15 3v18m-7-12 3 3-3 3" /></svg>
            </button>
            </>}
          </div>
        </header>
      {activeView === "quota" ? (
        <QuotaCenter views={quotaViews} installations={snapshot.installations} agentUpdates={agentUpdates} refreshProgress={quotaRefreshProgress} onRefresh={() => refreshQuotasNow.current()} onDetect={detectAgents} lang={resolvedLang} />
      ) : activeView === "remote" ? <RemoteDevices lang={resolvedLang} /> : <section className={`workspace ${showConversation ? "has-history" : "empty-workspace"}`}
        onDragEnter={event => { if (event.dataTransfer.types.includes("Files")) { event.preventDefault(); dragDepth.current++; setDraggingFiles(true); } }}
        onDragOver={event => { if (event.dataTransfer.types.includes("Files")) { event.preventDefault(); event.dataTransfer.dropEffect = sending ? "none" : "copy"; } }}
        onDragLeave={event => { event.preventDefault(); if (--dragDepth.current <= 0) { dragDepth.current = 0; setDraggingFiles(false); } }}
        onDrop={event => { event.preventDefault(); dragDepth.current = 0; setDraggingFiles(false); if (!sending) void attachments.add(Array.from(event.dataTransfer.files)); }}
        onPaste={event => { const files = Array.from(event.clipboardData.files); if (files.length) { event.preventDefault(); if (!sending) { void attachments.add(files); const text = event.clipboardData.getData("text/plain"); if (text) setDraft(previous => previous + text); } } }}>
        {draggingFiles && <div className="attachment-drop-overlay">{t("welcome.dropFiles", undefined, resolvedLang)}</div>}
        <input ref={attachmentInput} type="file" multiple hidden onChange={event => { if (!sending) void attachments.add(Array.from(event.target.files ?? [])); event.target.value = ""; }} />

        <PendingExecutorContext.Provider value={pendingExecutors}><AgentQuotaContext.Provider value={quotaViews}><ConversationPages transient={!selectedTaskId} taskId={showConversation ? selectedTask.taskId : null} revision={displayedTimeline} status={conversationStatus} latestUserMessageId={latestUserMessageId}>
          {<ConversationContent onForked={async taskId => {
            const next = await client.bootstrap();
            if (!next.tasks.some(task => task.taskId === taskId)) throw new Error("Fork 已创建，但暂时无法打开，请重试");
            previousTasks.current = next.tasks;
            setSnapshot(next);
            selectTask(taskId); setActiveView("conversation"); setSubagentItemId(null);
          }} onReviewFile={path => {
            setReviewFileRequest(previous => ({ taskId: selectedTask.taskId, path, id: (previous?.id ?? 0) + 1 }));
            setSubagentItemId(null); setReviewOpen(true);
            if (reviewWindow.detached) void reviewWindow.dock();
          }} renderAgent={renderResponseAgent} pendingAgent={pendingAgent} items={displayedTimeline.filter(item => item.kind !== "handoff" && item.kind !== "reconciliation")} client={client} activeTurnId={selectedTask.activePrimaryRun?.state === "disconnected" ? undefined : selectedTask.activePrimaryRun?.turnId} onSubagent={openSubagent} renderEntry={item => <TimelineEntry item={item} active={!!selectedTask.activePrimaryRun && selectedTask.activePrimaryRun.state !== "disconnected" && item.turnId === selectedTask.activePrimaryRun.turnId} key={item.itemId} />} />}
          </ConversationPages></AgentQuotaContext.Provider></PendingExecutorContext.Provider>
        {!showConversation && selectedTaskId && conversationLoader.pending ? <ConversationLoading visible={conversationLoader.visible} />
        : !showConversation && timelineError ? <div className="conversation-loading"><p role="alert">{timelineError} <button type="button" onClick={() => { setTimelineLoading(true); void loadConversation(selectedTask.taskId); }}>{t("welcome.retry", undefined, resolvedLang)}</button></p></div>
        : !showConversation ? <div className="welcome-stage">
          <img className="welcome-logo" src={appLogo} alt="" aria-hidden="true" draggable={false} />
          <h2>{t("welcome.greeting", undefined, resolvedLang)}</h2>
          <p>{readyAgentCount > 0 ? t("welcome.readyAgentSelected", { agent: providerLabel(selectedTask.preferredExecutor.providerId) }, resolvedLang) : t("welcome.noAgentAvailable", undefined, resolvedLang)}</p>
          {approvalPanel ?? <div className="welcome-composer">
            {!selectedTaskId && <div className="welcome-project-bar"><ProjectPicker projects={snapshot.projects} selectedProject={selectedProject}
              disabled={creatingTask || sending || Boolean(selectedTask.activePrimaryRun)}
              onSelect={projectId => { if (projectId !== selectedTask.projectId) void createConversation(projectId); }}
              onOpenFolder={() => void openProjectFolder()} /></div>}
            <form className="composer" onSubmit={submit}>
            {error && <p className="error" role="alert">{error}</p>}
            <AttachmentPreviews items={attachments.items} remove={attachments.remove} disabled={sending} />
            {attachments.error && <p className="error" role="alert">{attachments.error}</p>}
            <ComposerInput draft={draft} setDraft={setDraft} />
            <ComposerFooter
              lang={resolvedLang}
              modelPicker={modelPicker}
              permissionPicker={permissionPicker}
              submissionPending={submissionPending}
              modelSelectionPending={Boolean(activeModel) && (modelLoading || Boolean(modelError))}
              selectedTask={selectedTask}
              selectedQuota={selectedQuota}
              quotaDiagnosticCode={quotaViews.find(view => view.installationId === selectedTask.preferredExecutor.installationId)?.diagnostic}
              turnQuotaExhausted={selectedTurnQuotaExhausted}
              executorOptions={executorOptions}
              selectedInstallationStatus={selectedInstallation?.status}
              sending={sending || attachments.pending > 0}
              cancelling={cancelling}
              cancelPending={cancelPending}
              draft={draft || (attachments.items.length ? "附件" : "")}
                onAddAttachment={() => attachmentInput.current?.click()}
              onSelect={(executor) => void switchAgent(executor)}
              onCancel={() => void cancelActiveTurn()}
            />
            </form>
          </div>}
        </div> : <>

          <div className="composer-dock">
            {timelineError && <p role="alert">{timelineError} <button type="button" onClick={() => void loadConversation(selectedTask.taskId)}>{t("welcome.retry", undefined, resolvedLang)}</button></p>}
            {selectedTask.activePrimaryRun?.state === "disconnected" && <LoadingStatus>执行连接已中断，正在恢复会话…</LoadingStatus>}
            {approvalPanel ?? <form className="composer" onSubmit={submit}>

              {error && <p className="error" role="alert">{error}</p>}
              <AttachmentPreviews items={attachments.items} remove={attachments.remove} disabled={sending} />
              {attachments.error && <p className="error" role="alert">{attachments.error}</p>}
              <ComposerInput draft={draft} setDraft={setDraft} />
              <ComposerFooter
                lang={resolvedLang}
                modelPicker={modelPicker}
              permissionPicker={permissionPicker}
                submissionPending={submissionPending}
              modelSelectionPending={Boolean(activeModel) && (modelLoading || Boolean(modelError))}
                selectedTask={selectedTask}
                selectedQuota={selectedQuota}
                quotaDiagnosticCode={quotaViews.find(view => view.installationId === selectedTask.preferredExecutor.installationId)?.diagnostic}
                turnQuotaExhausted={selectedTurnQuotaExhausted}
                executorOptions={executorOptions}
                selectedInstallationStatus={selectedInstallation?.status}
                sending={sending || attachments.pending > 0}
                cancelling={cancelling}
                cancelPending={cancelPending}
                draft={draft || (attachments.items.length ? "附件" : "")}
                onAddAttachment={() => attachmentInput.current?.click()}
                onSelect={(executor) => void switchAgent(executor)}
                onCancel={() => void cancelActiveTurn()}
              />
            </form>}
          </div>
        </>}
        <AttachmentUploadRecovery client={client} busy={sending} attachedIds={attachments.items.map(item => item.id)} onRecover={attachments.recover}/>
        <SubmissionRecovery client={client} busy={sending} canResume={record => !agentPreferences.disabled.includes(record.target.providerId)
          && executorOptions.some(option => option.executor.providerId === record.target.providerId
            && (option.executor.installationId ?? null) === (record.target.installationId ?? null))} onResolved={(record, result) => {
          setPendingSubmission(current => current?.operationId === record.operationId
            ? result.status === "submitted" ? { ...current, taskId: result.taskId, acceptedSequence: result.sequence } : null : current);
          void refresh();
          if (result.status !== "task_deleted" && selectedTaskIdRef.current === record.task.taskId) void loadConversation(record.task.taskId);
        }} onUnconfirmedResolved={(record, result) => {
          setPendingSubmission(current => current?.operationId === record.operationId
            ? result.status === "submitted" ? { ...current, taskId: result.taskId, acceptedSequence: result.sequence } : null : current);
          void refresh();
          if (result.status === "submitted" && selectedTaskIdRef.current === result.taskId) void loadConversation(result.taskId);
        }} onRestore={record => {
          // Verified never received: give the text back only to the same, still-empty draft.
          const same = selectedTaskIdRef.current ? record.taskId === selectedTaskIdRef.current : record.scope === newDraftScope.current;
          if (same) setDraft(current => current.trim() ? current : record.input.prompt);
        }}/>
        {approvalError?.taskId === selectedTaskId && <p className="error" role="alert">{approvalError.message}</p>}
        {selectedTaskId && <ApprovalRecovery key={`approval:${selectedTaskId}`} client={client} taskId={selectedTaskId} busy={approving !== null} revision={selectedTask?.streamVersion ?? 0} onAccepted={record => {
          setApprovalError(current => current?.taskId === record.query.taskId && current.approval.approvalId === record.query.approvalId
            && current.approval.turnId === record.query.turnId && current.approval.requestedAtSequence === record.query.requestedAtSequence ? null : current);
          void refresh(); void loadConversation(selectedTaskId);
        }}/>}
        {selectedTaskId && <CancelRecovery key={selectedTaskId} client={client} taskId={selectedTaskId} busy={cancelling} onAccepted={() => { void refresh(); }}/>}
      </section>}
      {terminalLoaded && selectedProject && <Suspense fallback={activeView === "conversation" && terminalOpen ? <LoadingStatus>正在加载终端…</LoadingStatus> : null}><TerminalDock
        key={selectedProject.projectId} projectId={selectedProject.projectId}
        directories={[selectedProject.path, ...selectedProject.additionalDirectories]}
        open={activeView === "conversation" && terminalOpen} onClose={() => setTerminalOpen(false)}
        onEditProject={client.updateProject ? () => setEditingProject(selectedProject) : undefined} /></Suspense>}
      </div>

      {editingProject && <ProjectDialog key={editingProject.projectId} client={client} project={editingProject}
        onClose={() => setEditingProject(null)} onReady={async () => { await refresh(); setEditingProject(null); }} />}
      {projectDialogOpen && <ProjectDialog
        client={client}
        onClose={() => setProjectDialogOpen(false)}
        onReady={async (projectId) => {
          await refresh();
          setProjectDialogOpen(false);
          createConversation(projectId);
        }}
      />}
      {activeView === "conversation" && !reviewOpen && timeline.some(i => i.itemId === subagentItemId && i.taskId === selectedTask.taskId && activityOf(i)) && <SubagentPanel client={client} key={subagentItemId} item={timeline.find(i => i.itemId === subagentItemId)!} items={timeline} activeTurnId={selectedTask.activePrimaryRun?.state === "disconnected" ? undefined : selectedTask.activePrimaryRun?.turnId} onClose={() => setSubagentItemId(null)} />}
      {reviewWindow.error && <p className="review-window-error" role="alert">{reviewWindow.error}</p>}
      {activeView === "conversation" && selectedTaskId && reviewOpen && !reviewWindow.detached && <ReviewResizeHandle width={reviewWidth.width} max={reviewWidth.max} onResize={reviewWidth.setWidth} />}
      <PendingExecutorContext.Provider value={pendingExecutors}><AgentQuotaContext.Provider value={quotaViews}>
        <ConversationReviewDocks workspaceNavigation onOpenConversation={taskId => { selectTask(taskId as TaskId); setActiveView("conversation"); setSubagentItemId(null); }} tasks={snapshot.tasks} selectedTaskId={selectedTaskId} onDetach={reviewWindow.available && !reviewWindow.opening ? view => void reviewWindow.detach(view) : undefined} open={activeView === "conversation" && reviewOpen && !reviewWindow.detached} fileRequest={reviewFileRequest} onClose={() => setReviewOpen(false)}
          onEditProject={selectedProject && client.updateProject ? () => setEditingProject(selectedProject) : undefined} />
      </AgentQuotaContext.Provider></PendingExecutorContext.Provider>

    </main>
    </I18nProvider>
  );
}

function ComposerProject({ projectName, projectPath }: { projectName?: string; projectPath?: string }) {
  return (
    <div className="composer-project" data-tooltip={projectPath}>
      <span aria-hidden="true">▱</span>
      <b>{projectName ? projectLabel(projectName) : t("header.temporaryWorkspace")}</b>
      {projectName !== "快速对话" && projectPath && <small>{projectPath}</small>}
    </div>
  );
}

export function ComposerInput({ draft, setDraft }: { draft: string; setDraft: (value: string) => void }) {
  return (
    <textarea
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onKeyDown={(event) => {
        if (event.key !== "Enter" || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
        if (event.ctrlKey) {
          event.preventDefault();
          const input = event.currentTarget;
          const start = input.selectionStart;
          const end = input.selectionEnd;
          setDraft(input.value.slice(0, start) + "\n" + input.value.slice(end));
          requestAnimationFrame(() => input.setSelectionRange(start + 1, start + 1));
        } else if (!event.shiftKey && !event.altKey && !event.metaKey) {
          event.preventDefault();
          if (!event.repeat) event.currentTarget.form?.requestSubmit();
        }
      }}
      placeholder={t("composer.placeholder")}
      rows={4}
      autoFocus
    />
  );
}


export function quotaDiagnostic(code?: string | null): string {
  switch (code) {
    case "quota_network_error": return "额度查询网络异常，请检查网络和系统代理后重试。";
    case "quota_timeout": return "额度查询超时，请稍后重试或检查网络和系统代理。";
    case "quota_login_required": return "额度查询授权缺失、失效或被拒绝，请检查原生客户端登录状态或 API Key。";
    case "quota_subscription_required": return "当前账号没有有效订阅或套餐已到期，请在原生客户端确认订阅或切换账号、模型。";
    case "quota_access_denied": return "当前账号没有服务或模型访问权限，请检查账号资格及服务配置。";
    case "quota_not_configured": return "Agent 尚未配置好，请检查原生客户端的模型、服务及运行环境。";
    case "quota_exhausted": return "当前 Agent 额度已用尽，请等待重置或切换 Agent。";
    case "quota_rate_limited": return "额度接口请求暂时受限，请稍后重试。";
    case "claude_quota_custom_provider": return "Claude Code 当前使用自定义 API 服务，无法提供订阅额度监控。";
    case "claude_quota_login_required": return "未找到可用的 Claude 订阅授权，或授权已失效/缺少额度权限。请在 Claude Code 中使用订阅账号登录后重试；API Key 不提供此订阅额度。";
    case "claude_quota_transport_unavailable": return "额度查询需要本机 curl，目前无法启动。";
    case "claude_quota_rate_limited": return "Claude 额度接口请求受限，稍后自动重试。";
    case "claude_quota_token_expired": return "Claude Code 的短期访问令牌已到期，登录状态仍有效，无需重新登录。下次在终端或这里用 Claude Code 对话时会自动续期。";
    case "claude_quota_network_error":
    case "claude_quota_timeout": return "Claude 额度查询连接失败或超时，请检查网络和系统代理。";
    case "claude_quota_invalid_response":
    case "claude_quota_no_windows": return "Claude 接口未返回可识别的订阅额度窗口。";
    default: return "暂无额度读数（尚未上报、暂时不可用或不支持）";
  }
}

/// Small radial indicator for the selected agent's most restrictive quota
/// window; hover reveals every window plus context usage and cache hit rate.
export function QuotaRing({ quota, providerId, diagnostic, turnQuotaExhausted = false, inline = false }: { quota: QuotaSnapshotView["quota"]; providerId?: string; diagnostic?: string | null; turnQuotaExhausted?: boolean; inline?: boolean }) {
  const [quotaOpen, setQuotaOpen] = useState(false);
  const primary = ringQuotaWindow(quota?.windows ?? [], providerId);
  const primaryReset = primary ? quotaWindowHasReset(primary) : false;
  const remaining = primaryReset ? null : remainingPercent(primary?.usedPercent);
  if (inline && remaining === null) return null;
  const stale = quota ? quotaIsStale(quota.staleAfterUnixMs) : false;
  const hasFreshReportedWindows = Boolean(
    quota
      && quota.confidence === "provider_reported"
      && !stale
      && quota.windows.some((window) => !window.scope && !quotaWindowHasReset(window) && remainingPercent(window.usedPercent) !== null),
  );
  const exhausted = providerId === "antigravity" ? remaining === 0
    : quotaIsExhausted(quota) || (turnQuotaExhausted && !hasFreshReportedWindows);
  const color = exhausted ? "#d93025" : remaining === null ? "currentColor" : quotaColor(remaining);
  const radius = 8;
  const circumference = 2 * Math.PI * radius;
  const dash = ((remaining ?? 0) / 100) * circumference;
  const indicator = <>
      <svg width="18" height="18" viewBox="0 0 22 22" aria-hidden="true">
        <circle cx="11" cy="11" r={radius} fill="none" stroke="rgba(128,128,128,0.3)" strokeWidth="3" />
        <circle
          cx="11" cy="11" r={radius} fill="none"
          stroke={color} strokeWidth="3" strokeLinecap="round"
          strokeDasharray={`${dash} ${circumference}`}
          transform="rotate(-90 11 11)"
        />
      </svg>
      <span className="quota-ring-value" aria-hidden="true">
        {exhausted ? "!" : remaining === null ? "?" : `${remaining > 0 && remaining < 1 ? "<1" : Math.round(remaining)}%`}
      </span>
  </>;
  if (inline) {
    const rows: [string, string][] = [];
    if (providerId === "antigravity") rows.push(["圆环", "Gemini · 5 小时"]);
    if (providerId === "cursor" && primary?.id === "cursor-models") rows.push(["圆环", "Cursor Models · 每月"]);
    const plan = quota?.planType && quota.planType !== "demo" ? quotaPlanLabel(quota) : null;
    if (plan) rows.push(["套餐", plan]);
    if (exhausted) rows.push(["状态", providerId === "antigravity" ? "Gemini 5 小时额度已用尽" : "额度已用尽"]);
    if (!quota?.windows.length) rows.push(["状态", quotaDiagnostic(diagnostic)]);
    else if (quotaShowsLastReading(diagnostic)) rows.push(["状态", `${quotaDiagnostic(diagnostic)}当前显示上次读数。`]);
    for (const window of quota?.windows ?? []) {
      const label = [quotaScopeLabel(window.scope), quotaWindowLabel(window.windowMinutes, window.id)].filter(Boolean).join(" · ");
      const balance = remainingPercent(window.usedPercent);
      rows.push([label, quotaWindowHasReset(window) ? "已到重置时间，待刷新" : balance === null ? "用量未知" : balance === 0 ? "已用尽" : `${balance}%`]);
      const reset = window.resetsAtUnix ? formatResetTime(window.resetsAtUnix) : nativeResetDisplay(quota!, window);
      if (reset) rows.push([`${label} · 重置`, reset]);
    }
    if (quota) rows.push(["更新于", new Date(quota.collectedAtUnixMs).toLocaleTimeString()]);
    return <span className={`quota-ring${exhausted ? " exhausted" : ""}`} data-tooltip="剩余额度" data-tooltip-rows={JSON.stringify(rows)}>
      {indicator}
    </span>;
  }
  return (
    <div onMouseEnter={() => setQuotaOpen(true)} onMouseLeave={() => setQuotaOpen(false)} onFocus={() => setQuotaOpen(true)} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) setQuotaOpen(false); }} onKeyDown={event => { if (event.key === "Escape") setQuotaOpen(false); }} className={`quota-ring ${exhausted ? "exhausted" : ""}`} tabIndex={0} aria-label={exhausted ? "Coding Agent 额度已用尽，查看详情" : remaining === null ? "额度暂无读数，查看详情" : `${providerId === "cursor" && primary?.scope ? `${primary.scope} · ` : ""}${quotaWindowLabel(primary!.windowMinutes, primary!.id)}剩余额度 ${remaining}%`}>
      {indicator}
      <FloatingPopover open={quotaOpen} className="quota-popover" role="tooltip">
        <header>
          <b>剩余额度</b>
          {providerId === "antigravity" && <small>圆环：Gemini · 5 小时</small>}
          {providerId === "cursor" && primary?.id === "cursor-models" && <small>圆环：Cursor Models · 每月</small>}
          {quota?.planType && quota.planType !== "demo" && <small>{quotaPlanLabel(quota)}</small>}
          {primaryReset && <small className="quota-stale">已到重置时间，等待读取新额度</small>}
          {!!quota?.windows.length && quotaShowsLastReading(diagnostic) && <small className="quota-stale">{quotaDiagnostic(diagnostic)}当前显示上次读数。</small>}
        </header>
        {exhausted && <p className="quota-exhausted">{providerId === "antigravity" ? "Gemini 5 小时额度已用尽。" : "额度已用尽。请等待额度重置或切换其他 Agent。"}</p>}
        {!primary && quota?.windows.some((window) => window.scope) && <p>额度按模型组计算，当前模型组未确定，请查看下方分组读数。</p>}
        {(!quota || quota.windows.length === 0) && <p>{diagnostic ? quotaDiagnostic(diagnostic) : "暂无订阅额度读数。部分 Agent 仅在对话后上报，或不提供此数据。"}</p>}
        {quota?.windows.map((window) => {
          const remaining = remainingPercent(window.usedPercent);
          return (
            <div className="quota-row" key={window.id}>
              <span className="quota-window-label">{quotaScopeLabel(window.scope) ? `${quotaScopeLabel(window.scope)} · ` : ""}{quotaWindowLabel(window.windowMinutes, window.id)}</span>
              <b>{remaining == null ? "用量未知" : remaining === 0 ? "已用尽" : `${remaining}%`}</b>
              <small>
                {window.resetsAtUnix ? `重置时间：${formatResetTime(window.resetsAtUnix)}` : quotaNativeResetLabel(quota, window)}
              </small>
            </div>
          );
        })}
        {quota?.context && (
          <>
            <div className="quota-row">
              <span>上下文</span>
              <b>
                {(quota.context.usedTokens / 1000).toFixed(1)}k/
                {(quota.context.windowTokens / 1000).toFixed(0)}k
              </b>
              <small>
                {Math.round((quota.context.usedTokens / quota.context.windowTokens) * 100)}%
              </small>
            </div>
            {quota.context.cacheHitRatePercent != null && (
              <div className="quota-row">
                <span>缓存命中</span>
                <b>{quota.context.cacheHitRatePercent.toFixed(1)}%</b>
              </div>
            )}
          </>
        )}
        {quota && <div className="quota-source">更新于 {new Date(quota.collectedAtUnixMs).toLocaleTimeString()}</div>}
      </FloatingPopover>
    </div>
  );
}

/// Overall quota panel: every connected agent with a quota source.
export function QuotaPanel({
  views,
  installations = [],
  agentUpdates,
  management = false,
  onDetect,
}: {
  management?: boolean;
  /** Scans for the given agents again, so ones installed after first launch can be enabled. */
  onDetect?: (providerIds: string[]) => Promise<void>;
  views: QuotaSnapshotView[];
  installations?: Array<Pick<InstallationSummary, "installationId" | "version"> & Partial<Pick<InstallationSummary, "providerId" | "status">>>;
  agentUpdates?: AgentUpdateActions;
}) {
  const preferences = useAgentPreferences();
  const [dragged, setDragged] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [detecting, setDetecting] = useState(false);
  const installedViews = management ? installations.filter(item => item.providerId && item.providerId !== "fake").map(item =>
    views.find(view => view.installationId === item.installationId) ?? { providerId: item.providerId!, installationId: item.installationId, quota: null, diagnostic: null }) : visibleQuotaViews(views, installations);
  // Only installed agents take part in ordering; known but missing ones are listed compactly below.
  const uninstalled = management ? supportedAgents.filter(agent => !installedViews.some(view => view.providerId === agent.id)) : [];
  const visible = orderAgents(installedViews, item => item.providerId, preferences);
  const providerOrder = visible.map(view => view.providerId);
  const endDrag = () => { setDragged(null); setDropTarget(null); };
  const versionFor = (installationId: string) => {
    const engine = installations.find((installation) => installation.installationId === installationId)?.version;
    const desktop = agentUpdates?.updates[installationId]?.desktopVersion;
    const provider = visible.find(view => view.installationId === installationId)?.providerId;
    if (desktop) return `客户端 ${desktop} · 引擎 ${engine ?? "版本未知"}`;
    return `${provider === "codex" || provider === "zcode" ? "引擎 " : ""}${engine ?? "版本未知"}`;
  };
  const cards = <div className="quota-panel agent-management-grid">
    {visible.map(view => {
      const quota = view.quota;
      const installed = installations.some(item => item.installationId === view.installationId);
      const installation = installations.find(item => item.installationId === view.installationId);
      const loginAgent = supportedAgents.find(agent => agent.id === view.providerId);
      const accountIssue = agentAccountIssue(installation, view);
      const loginRequired = management && loginAgent && needsAgentLogin(installation, view);
      const enabled = !preferences.disabled.includes(view.providerId);
      return <section className="agent-management-card" key={view.installationId} aria-label={providerLabel(view.providerId)}
        data-disabled={management && !enabled || undefined} data-dragging={dragged === view.providerId || undefined} data-drop-target={dropTarget === view.providerId || undefined}
        onDragOver={management ? event => { if (!dragged || dragged === view.providerId || !event.dataTransfer.types.includes(AGENT_DRAG_TYPE)) return; event.preventDefault(); event.dataTransfer.dropEffect = "move"; setDropTarget(view.providerId); } : undefined}
        onDrop={management ? event => { if (!dragged || event.dataTransfer.getData(AGENT_DRAG_TYPE) !== dragged) return; event.preventDefault(); moveAgent(dragged, view.providerId, providerOrder); endDrag(); } : undefined}>
        {management && <AgentManagementControls providerId={view.providerId} enabled={enabled} order={providerOrder}
          onDragStart={event => { event.dataTransfer.setData(AGENT_DRAG_TYPE, view.providerId); event.dataTransfer.effectAllowed = "move"; setDragged(view.providerId); }} onDragEnd={endDrag} />}
        <header className="agent-card-heading">
          <AgentIcon providerId={view.providerId} />
          <div className="agent-card-identity"><h3>{providerLabel(view.providerId)}</h3>
            {loginRequired ? <span className="agent-card-login-status">{accountIssue!.label}<AgentLogin installationId={view.installationId} name={loginAgent.name} /></span>
              : <span>{accountIssue?.label ?? (quota ? quotaPlanLabel(quota) || "订阅额度" : "额度暂不可用")}</span>}
          </div>
          {quota && <time className="agent-card-updated" dateTime={new Date(quota.collectedAtUnixMs).toISOString()} data-tooltip={new Date(quota.collectedAtUnixMs).toLocaleString()}>更新于 {new Date(quota.collectedAtUnixMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time>}
        </header>
        <div className="agent-card-content">
          {quota ? <>
            {quota.windows.length > 0 && <div className="agent-quota-caption">剩余额度</div>}
            <div className="agent-quota-windows">
              {quota.windows.map(window => {
                const reset = quotaWindowHasReset(window);
                const remaining = reset ? null : remainingPercent(window.usedPercent);
                const label = `${quotaScopeLabel(window.scope) ? `${quotaScopeLabel(window.scope)} · ` : ""}${quotaWindowLabel(window.windowMinutes, window.id)}`;
                const resetLabel = window.resetsAtUnix ? `重置时间：${formatResetTime(window.resetsAtUnix)}` : quotaNativeResetLabel(quota, window);
                return <div className="agent-quota-window" key={window.id}>
                  <div className="agent-quota-label"><span>{label}</span><strong style={remaining === 0 ? { color: quotaColor(0) } : undefined}>{reset ? "待刷新" : remaining == null ? "未知" : `${remaining}%`}</strong></div>
                  <div className="agent-quota-meter" role={remaining == null ? undefined : "progressbar"} aria-label={`${label}剩余额度`} aria-valuemin={remaining == null ? undefined : 0} aria-valuemax={remaining == null ? undefined : 100} aria-valuenow={remaining ?? undefined} data-unknown={remaining == null || undefined}>
                    {remaining != null && <i style={{ width: `${remaining}%`, background: quotaColor(remaining) }} />}
                  </div>
                  <small className="agent-quota-reset">{reset ? "已到重置时间，待刷新" : remaining == null ? "用量未知" : remaining === 0 ? "已用尽" : ""}{!reset && resetLabel && <span>{resetLabel}</span>}</small>
                </div>;
              })}
            </div>
            {(!quota.windows.length || quotaShowsLastReading(view.diagnostic)) && <p className="agent-quota-notice" data-outdated={quota.windows.length > 0 || undefined}>{quotaDiagnostic(view.diagnostic)}{quota.windows.length > 0 && "当前显示上次读数。"}</p>}
            {quota.context && <div className="agent-context"><span>当前会话上下文</span><b>{quota.context.usedTokens.toLocaleString()} / {quota.context.windowTokens.toLocaleString()}</b>{quota.context.cacheHitRatePercent != null && <small>缓存命中 {quota.context.cacheHitRatePercent.toFixed(1)}%</small>}</div>}
          </> : <p className="agent-quota-notice">{quotaDiagnostic(view.diagnostic)}</p>}
        </div>
        {installed && <footer className="agent-card-footer">
          <div className="agent-card-version" data-tooltip={versionFor(view.installationId)} data-tooltip-overflow="">{versionFor(view.installationId)}</div>
          <AgentUpdateControl id={view.installationId} actions={agentUpdates} />
        </footer>}
      </section>;
    })}
  </div>;
  if (!uninstalled.length) return cards;
  return <>{cards}
    <section className="agent-uninstalled" aria-labelledby="agent-uninstalled-title">
      <header><h3 id="agent-uninstalled-title">未安装</h3><span>安装后重新检测即可启用</span>
        {onDetect && <button className="agent-detect-button" type="button" disabled={detecting} aria-busy={detecting}
          onClick={() => { setDetecting(true); void onDetect(uninstalled.map(agent => agent.id)).finally(() => setDetecting(false)); }}>
          {detecting && <span className="task-running-spinner" aria-hidden="true" />}{detecting ? "正在检测…" : "重新检测"}
        </button>}
      </header>
      <ul>{uninstalled.map(agent => <li key={agent.id} aria-label={agent.name}>
        <AgentIcon providerId={agent.id} /><span className="agent-uninstalled-name">{agent.name}</span>
        <InstallationLink url={agent.installUrl} name={agent.name} className="agent-install-link" errorClassName="agent-install-error" />
      </li>)}</ul>
    </section></>;
}

export function QuotaCenter({
  views,
  installations,
  agentUpdates,
  refreshProgress,
  onRefresh,
  onDetect,
  lang,
}: {
  views: QuotaSnapshotView[];
  installations: InstallationSummary[];
  agentUpdates: AgentUpdateActions;
  refreshProgress: QuotaRefreshProgress | null;
  onRefresh: () => void;
  onDetect?: (providerIds: string[]) => Promise<void>;
  lang?: ResolvedLanguage;
}) {
  return (
    <section className="workspace quota-center agent-management" aria-label={t("agentManagement.title", undefined, lang)}>

      <div className="quota-center-content">
        <div className="quota-center-intro">
          <p className="agent-management-description">{t("agentManagement.description", undefined, lang)}</p>
          <button className="quota-refresh-button" type="button" disabled={refreshProgress?.running}
            onClick={onRefresh} aria-busy={refreshProgress?.running ?? false}>
            {refreshProgress?.running ? <span className="task-running-spinner" aria-hidden="true" /> : <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5M6 7a7 7 0 0 1 12-1l2 6M4 12l2 6a7 7 0 0 0 12-1" /></svg>}
            {refreshProgress?.running ? t("agentManagement.refreshing", undefined, lang) : t("agentManagement.refreshQuota", undefined, lang)}
          </button>
        </div>
        {refreshProgress && <p className="quota-refresh-status" role="status">
          {refreshProgress.running
            ? refreshProgress.total ? t("agentManagement.progress", { completed: refreshProgress.completed, total: refreshProgress.total }, lang) : t("agentManagement.fetchingList", undefined, lang)
            : refreshProgress.failed ? t("agentManagement.refreshFailed", undefined, lang)
            : !refreshProgress.total ? t("agentManagement.noRefreshable", undefined, lang)
            : refreshProgress.unavailable ? `已完成刷新，${refreshProgress.unavailable} 个 Agent 未取得新额度；已有读数请以原更新时间为准。`
            : t("agentManagement.refreshSuccess", { count: refreshProgress.total }, lang)}
        </p>}
        <div className="agent-management-summary"><span>{t("agentManagement.installedCount", { count: installations.filter(item => item.providerId !== "fake").length }, lang)}</span><span>{t("agentManagement.quotaCalculation", undefined, lang)}</span></div>
        <QuotaPanel views={views} installations={installations} agentUpdates={agentUpdates} onDetect={onDetect} management />
      </div>
    </section>
  );
}

export function ComposerFooter({
  lang,
  permissionPicker,
  modelPicker,
  modelSelectionPending = false,
  submissionPending = false,
  selectedTask,
  selectedQuota,
  quotaDiagnosticCode,
  turnQuotaExhausted,
  executorOptions,
  selectedInstallationStatus,
  sending,
  cancelling,
  cancelPending = false,
  draft,
  onSelect,
  onCancel,
  onAddAttachment,
}: {
  onAddAttachment?: () => void;
  lang?: ResolvedLanguage;
  permissionPicker?: ReactNode;
  modelPicker?: ReactNode;
  modelSelectionPending?: boolean;
  submissionPending?: boolean;
  selectedTask: TaskSummary;
  selectedQuota: QuotaSnapshotView["quota"];
  quotaDiagnosticCode?: string | null;
  turnQuotaExhausted: boolean;
  executorOptions: ExecutorOption[];
  selectedInstallationStatus?: InstallationStatus;
  sending: boolean;
  cancelling: boolean;
  cancelPending?: boolean;
  draft: string;
  onSelect: (executor: ExecutorRef) => void;
  onCancel: () => void;
}) {
  const quotaBlocked = quotaIsExhausted(selectedQuota);
  const preferences = useAgentPreferences();
  const agentDisabled = preferences.disabled.includes(selectedTask.preferredExecutor.providerId) || (preferences.disabled.length > 0 && !executorOptions.length);
  const footerRef = useComposerLabels();
  return (
    <footer ref={footerRef} className="composer-controls">
      <button className="composer-icon" type="button" aria-label={t("composer.addAttachment", undefined, lang)} disabled={sending} onClick={onAddAttachment}><svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 4v12M4 10h12" /></svg></button>
      <Fragment key="permission-control">{permissionPicker ?? <span className="mode-chip">{t("composer.localExecution", undefined, lang)}</span>}</Fragment>
      <div className="spacer" />
      <ContextUsageRing quota={selectedQuota} providerId={selectedTask.preferredExecutor.providerId} />
      <Fragment key="model-control">{modelPicker}</Fragment>
      <AgentPicker
        current={selectedTask.preferredExecutor}
        options={executorOptions}
        onSelect={onSelect}
        lang={lang}
        quotaIndicator={<QuotaRing inline
          quota={selectedQuota}
          providerId={selectedTask.preferredExecutor.providerId}
          diagnostic={quotaDiagnosticCode}
          turnQuotaExhausted={turnQuotaExhausted}
        />}
      />
      {selectedTask.activePrimaryRun ? (
        <button
          className="stop composer-submit"
          type="button"
          disabled={cancelling || cancelPending || ["interrupting", "disconnected"].includes(selectedTask.activePrimaryRun.state)}
          onClick={onCancel}
          aria-label={cancelPending ? t("composer.stopPending", undefined, lang) : t("composer.stop", undefined, lang)}
        >
          {cancelling || selectedTask.activePrimaryRun.state === "interrupting" ? "…" : <svg viewBox="0 0 20 20" aria-hidden="true"><rect x="5" y="5" width="10" height="10" rx="1" fill="currentColor" stroke="none" /></svg>}
        </button>
      ) : (
        <button
          className="send composer-submit"
          aria-label={quotaBlocked ? t("composer.quotaExhausted", undefined, lang) : t("composer.send", undefined, lang)}
          data-tooltip={agentDisabled ? t("composer.enableInManagement", undefined, lang) : quotaBlocked ? t("composer.quotaExhausted", undefined, lang) : t("composer.tooltipShortcuts", undefined, lang)}
          disabled={!draft.trim() || sending || submissionPending || quotaBlocked || modelSelectionPending || agentDisabled}
        >
          {sending ? "…" : <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 16V4m-5 5 5-5 5 5" /></svg>}
        </button>
      )}
    </footer>
  );
}

function AgentPicker({
  current,
  options,
  onSelect,
  quotaIndicator,
  lang,
}: {
  current: ExecutorRef;
  options: ExecutorOption[];
  onSelect: (executor: ExecutorRef) => void;
  quotaIndicator?: ReactNode;
  lang?: ResolvedLanguage;
}) {
  const detailsRef = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    const dismissOutside = (event: PointerEvent) => {
      const details = detailsRef.current;
      if (details?.open && !details.contains(event.target as Node)) details.open = false;
    };
    const dismissEscape = (event: KeyboardEvent) => {
      const details = detailsRef.current;
      if (event.key === "Escape" && details?.open) {
        details.open = false;
        details.querySelector("summary")?.focus();
      }
    };
    document.addEventListener("pointerdown", dismissOutside, true);
    document.addEventListener("keydown", dismissEscape);
    return () => {
      document.removeEventListener("pointerdown", dismissOutside, true);
      document.removeEventListener("keydown", dismissEscape);
    };
  }, []);
  return (
    <details className="agent-picker" ref={detailsRef}>
      <summary aria-label={t("composer.selectAgent", undefined, lang)} data-tooltip={t("composer.currentAgent", { name: providerLabel(current.providerId) }, lang)}>
        <AgentIcon providerId={current.providerId} size="small" />
        <b className="composer-control-label">{providerLabel(current.providerId)}</b>
        {quotaIndicator}
        <DropdownChevron />
      </summary>
      <AgentPickerMenu options={options} selectedKey={executorKey(current)} title={t("composer.nextRoundAgent", undefined, lang)} label="Coding Agent 列表" onSelect={option => {
        detailsRef.current?.removeAttribute("open");
        if (option.key !== executorKey(current)) onSelect(option.executor);
        focusComposerInput(detailsRef.current);
      }}/>

    </details>
  );
}
