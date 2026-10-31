import { useSharedHistoryPreferences, retrySharedHistoryPreferences } from "./sharedHistoryPreferences";
import { useEffect, useId, useMemo, useRef, useState, type DragEvent, type HTMLAttributes } from "react";
import type { ProjectSummary, TaskId, TaskSummary } from "@dogcodes/protocol";
import { loadHistoryPreferences, saveHistoryPreference, type HistoryPreferences } from "./historyPreferences";
import { HistorySection } from "./HistorySection";
import { loadSectionPreferences, saveSectionPreferences, withSharedHistory, COLLAPSED_STORAGE_KEY, SECTION_STORAGE_KEY, orderHistory, historyTime, moveHistoryItem, type HistorySectionId, type HistorySort, type SectionPreferences } from "./historySections";
import { groupTasksByWorkspace } from "./workspaceGroups";

import { ProjectHoverCard } from "./ProjectHoverCard";
import { DeleteConversation } from "./DeleteConversation";
import { AgentIcon, providerLabel } from "./agentPresentation";

import { loadProjectHistoryPreferences, saveProjectHistoryPreference, PROJECT_HISTORY_KEY } from "./projectHistoryPreferences";
import { translate, useI18n } from "./i18n";
import { projectDisplayName, taskDisplayTitle } from "./displayNames";

const PREVIEW_COUNT = 5;

function HistoryAgent({ task }: { task: TaskSummary }) {
  const executor = task.activePrimaryRun?.executor ?? task.lastTurnExecutor ?? task.preferredExecutor;
  const label = translate(task.activePrimaryRun || task.lastTurnExecutor ? "history.lastTurnAgent" : "history.conversationAgent", { agent: providerLabel(executor.providerId) });
  return <span className="history-agent" role="img" aria-label={label} data-tooltip={label}>
    <AgentIcon providerId={executor.providerId} size="small" />
  </span>;
}

export function ConversationHistory({ projects, tasks, selectedTaskId, onSelect, onRename, onDelete, onEditProject, onCreateProject, onRevealProject, onCreateProjectConversation, createConversationDisabled, activeReviewTasks = [], selectionVisible = true }: {
  projects: ProjectSummary[];
  tasks: TaskSummary[];
  selectedTaskId: TaskId | null;
  onSelect: (taskId: TaskId) => void;
  onRename?: (task: TaskSummary, title: string) => Promise<void>;
  onDelete?: (task: TaskSummary) => Promise<void>;
  onEditProject?: (project: ProjectSummary) => void;
  onCreateProject?: () => void;
  onRevealProject?: (project: ProjectSummary) => Promise<void>;
  onCreateProjectConversation?: (project: ProjectSummary) => void;
  createConversationDisabled?: boolean;
  activeReviewTasks?: TaskId[];
  selectionVisible?: boolean;
}) {
  const [localPreferences, setPreferences] = useState<HistoryPreferences>(() => {
    try { return loadHistoryPreferences(); } catch { return {}; }
  });
  const [localProjectPreferences, setProjectPreferences] = useState(loadProjectHistoryPreferences);
  const [localSectionPreferences, setSectionPreferences] = useState(loadSectionPreferences);
  const sharedHistory = useSharedHistoryPreferences();
  const sharedValue = sharedHistory?.snapshot?.preferences;
  const preferences = sharedValue?.tasks ?? localPreferences;
  const projectPreferences = sharedValue?.projects ?? localProjectPreferences;
  const sectionPreferences = sharedValue ? withSharedHistory(localSectionPreferences, sharedValue) : localSectionPreferences;
  const preferencesBusy = !!sharedHistory && (sharedHistory.busy || sharedHistory.unconfirmed);
  const dragging = useRef<{ scope: string; id: string } | null>(null);
  const [dropTarget, setDropTarget] = useState<{ scope: string; id: string; edge: "before" | "after" } | null>(null);
  const finishDrag = () => { dragging.current = null; setDropTarget(null); };
  const dropEdge = (scope: string, id: string) => dropTarget?.scope === scope && dropTarget.id === id ? dropTarget.edge : undefined;
  useEffect(() => {
    const sync = (event: StorageEvent) => {
      if (event.key === PROJECT_HISTORY_KEY || event.key === null) setProjectPreferences(loadProjectHistoryPreferences());
      if (event.key === SECTION_STORAGE_KEY || event.key === COLLAPSED_STORAGE_KEY || event.key === null) setSectionPreferences(loadSectionPreferences()); };
    window.addEventListener("storage", sync);
    return () => window.removeEventListener("storage", sync);
  }, []);
  const deletionBlocked = (task: TaskSummary) => !!task.activePrimaryRun || activeReviewTasks.includes(task.taskId);
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<TaskId | null>(null);
  const [title, setTitle] = useState("");
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [renameError, setRenameError] = useState<string | null>(null);
  const renameErrorId = useId();
  const renameInput = useRef<HTMLInputElement>(null);
  const renameButtons = useRef(new Map<TaskId, HTMLButtonElement>());
  const returnFocusTo = useRef<TaskId | null>(null);
  const composing = useRef(false);
  useEffect(() => {
    if (renaming) { renameInput.current?.focus(); renameInput.current?.select(); }
    else if (returnFocusTo.current) {
      renameButtons.current.get(returnFocusTo.current)?.focus();
      returnFocusTo.current = null;
    }
  }, [renaming]);
  useEffect(() => {
    if (renameError && !saving) renameInput.current?.focus();
  }, [renameError, saving]);
  const renameButtonRef = (taskId: TaskId) => (button: HTMLButtonElement | null) => {
    if (button) renameButtons.current.set(taskId, button);
    else renameButtons.current.delete(taskId);
  };
  const finishRename = (restoreFocus = true) => {
    if (!restoreFocus) returnFocusTo.current = null;
    composing.current = false; setRenaming(null); setRenameError(null);
  };
  const beginRename = (task: TaskSummary) => {
    if (savingRef.current) return;
    returnFocusTo.current = task.taskId; composing.current = false;
    setRenaming(task.taskId); setTitle(task.title); setError(null); setRenameError(null);
  };
  const saveRename = async (task: TaskSummary) => {
    if (!onRename || savingRef.current || composing.current) return;
    if (!title.trim() || Array.from(title.trim()).length > 120 || /[\u0000-\u001f\u007f-\u009f]/u.test(title)) {
      setRenameError(translate("history.renameInvalid")); return;
    }
    if (title.trim() === task.title) { finishRename(false); return; }
    savingRef.current = true; setSaving(true); setRenameError(null);
    try { await onRename(task, title.trim()); finishRename(false); }
    catch { setRenameError(translate("history.renameFailed")); }
    finally { savingRef.current = false; setSaving(false); }
  };
  const renameForm = (task: TaskSummary) => <form className="history-rename" key={task.taskId}
    aria-label={translate("nav.rename")} aria-busy={saving} onSubmit={event => { event.preventDefault(); void saveRename(task); }}
    onKeyDown={event => {
      if (composing.current || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) {
        if (event.key === "Enter") event.preventDefault();
        return;
      }
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (!savingRef.current) finishRename(); }
    }}>
    <div className="history-rename-controls">
      <input ref={renameInput} aria-label={translate("history.nameLabel")} data-tooltip={translate("history.nameTooltip")} value={title} disabled={saving}
        aria-invalid={!!renameError} aria-describedby={renameError ? renameErrorId : undefined}
        onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
        onChange={event => { setTitle(event.target.value); setRenameError(null); }} />
      <button className="history-rename-save" type="submit" aria-label={translate(saving ? "history.saving" : "common.save")} data-tooltip={translate(saving ? "common.saving" : "history.saveTooltip")} disabled={saving || !title.trim()}>
        {saving ? <span className="task-running-spinner" aria-hidden="true" /> : <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m4 10 4 4 8-8" /></svg>}
      </button>
      <button type="button" aria-label={translate("common.cancel")} data-tooltip={translate("history.cancelTooltip")} disabled={saving} onClick={() => finishRename()}>
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m5 5 10 10M15 5 5 15" /></svg>
      </button>
    </div>
    {renameError && <p className="history-rename-error" id={renameErrorId} role="alert">{renameError}</p>}
  </form>;
  // Manual mode retains the original creation order until explicitly reordered.
  const newestFirst = useMemo(() => [...tasks].reverse(), [tasks]);
  const taskTime = (task: TaskSummary) => historyTime(task.updatedAt ?? task.activePrimaryRun?.startedAt);
  const sortTasks = (items: TaskSummary[], section: HistorySectionId, scope = section as string) =>
    orderHistory(items, sectionPreferences.sections[section].sort, sectionPreferences.orders[scope], task => task.taskId, taskTime);
  const projectTimes = new Map(projects.map(project => [project.projectId, historyTime(project.updatedAt)]));
  for (const task of tasks) projectTimes.set(task.projectId, Math.max(projectTimes.get(task.projectId) ?? 0, taskTime(task)));
  const orderedProjects = orderHistory(projects, sectionPreferences.sections.projects.sort, sectionPreferences.orders.projects, project => project.projectId, project => projectTimes.get(project.projectId) ?? 0);
  const visibleProjects = orderedProjects.filter(project => projectPreferences[project.projectId] !== "removed")
    .sort((a, b) => Number(projectPreferences[b.projectId] === "pinned") - Number(projectPreferences[a.projectId] === "pinned"));
  const removedProjects = projects.filter(project => projectPreferences[project.projectId] === "removed");
  const updateProjectPreference = (id: string, state: "pinned" | "removed" | null) => {
    try { setProjectPreferences(saveProjectHistoryPreference(id, state)); setError(null); }
    catch { setError(translate("history.projectSettingsFailed")); }
  };
  const groups = groupTasksByWorkspace(visibleProjects, newestFirst.filter(task => !preferences[task.taskId]))
    .map(group => ({ ...group, tasks: sortTasks(group.tasks, "projects", `project:${group.key}`) }));
  const pinned = sortTasks(newestFirst.filter(task => preferences[task.taskId] === "pinned"), "pinned");
  const archived = sortTasks(newestFirst.filter(task => preferences[task.taskId] === "archived"), "archived");
  const commitSections = (next: SectionPreferences) => {
    try { setSectionPreferences(saveSectionPreferences(next, sectionPreferences)); setError(null); return true; }
    catch { setError(translate("history.groupSettingsFailed")); return false; }
  };
  const sectionProps = (id: HistorySectionId) => ({
    preference: sectionPreferences.sections[id],
    sortDisabled: preferencesBusy,
    onToggle: () => commitSections({ ...sectionPreferences, sections: { ...sectionPreferences.sections, [id]: { ...sectionPreferences.sections[id], collapsed: !sectionPreferences.sections[id].collapsed } } }),
    onSort: (sort: HistorySort) => commitSections({ ...sectionPreferences, sections: { ...sectionPreferences.sections, [id]: { ...sectionPreferences.sections[id], sort } } }),
  });
  const move = (scope: string, ids: string[], source: string, target: string, edge?: "before" | "after") => commitSections({
    ...sectionPreferences, orders: { ...sectionPreferences.orders, [scope]: moveHistoryItem(ids, source, target, edge) },
  });
  const canDrop = (scope: string, id: string) => dragging.current?.scope === scope && dragging.current.id !== id
    // Pinned projects always precede other projects, even in manual order.
    && (scope !== "projects" || (projectPreferences[dragging.current.id] === "pinned") === (projectPreferences[id] === "pinned"));
  const insertionEdge = (event: DragEvent<HTMLElement>, scope: string): "before" | "after" => {
    // A project's header and its conversations share one drop boundary.
    const target = scope === "projects" ? event.currentTarget.closest(".project-group")! : event.currentTarget;
    const bounds = target.getBoundingClientRect();
    return event.clientY < bounds.top + bounds.height / 2 ? "before" : "after";
  };
  const dragProps = (scope: string, id: string, ids: string[], section: HistorySectionId): HTMLAttributes<HTMLElement> => preferencesBusy || sectionPreferences.sections[section].sort !== "manual" ? {} : {
    draggable: true,
    onDragStart: event => { event.stopPropagation(); setDropTarget(null); dragging.current = { scope, id }; event.dataTransfer.effectAllowed = "move"; event.dataTransfer.setData("text/plain", id); },
    onDragEnd: finishDrag,
    onDragOver: event => {
      if (!canDrop(scope, id)) return;
      event.preventDefault(); event.stopPropagation(); event.dataTransfer.dropEffect = "move";
      const edge = insertionEdge(event, scope);
      setDropTarget(current => current?.scope === scope && current.id === id && current.edge === edge ? current : { scope, id, edge });
    },
    onDrop: event => {
      if (!canDrop(scope, id)) return;
      event.preventDefault(); event.stopPropagation();
      move(scope, ids, dragging.current!.id, id, insertionEdge(event, scope)); finishDrag();
    },
    onKeyDown: event => {
      if (!event.altKey || !["ArrowUp", "ArrowDown"].includes(event.key) || (event.target as HTMLElement).closest("input, textarea")) return;
      const target = ids[ids.indexOf(id) + (event.key === "ArrowUp" ? -1 : 1)];
      event.preventDefault(); event.stopPropagation(); if (target) move(scope, ids, id, target);
    },
  };
  const update = (task: TaskSummary, status: "pinned" | "archived" | null) => {
    try {
      setPreferences(saveHistoryPreference(task.taskId, status));
      setError(null);
    } catch {
      setError(translate("history.listSettingsFailed"));
    }
  };
  const { t } = useI18n();
  const row = (task: TaskSummary, isPinned = false, scope = "pinned", ids = pinned.map(item => item.taskId)) => renaming === task.taskId ? renameForm(task) : <div className={selectionVisible && task.taskId === selectedTaskId ? "history-row active" : "history-row"} key={task.taskId} data-drop-edge={dropEdge(scope, task.taskId)} {...dragProps(scope, task.taskId, ids, isPinned ? "pinned" : "projects")}>
    <button type="button" className="task" data-tooltip={taskDisplayTitle(task.title)} data-tooltip-overflow=".task-title"
      aria-current={selectionVisible && task.taskId === selectedTaskId ? "page" : undefined} onClick={() => onSelect(task.taskId)}>
      <HistoryAgent task={task} />
      <span className="task-title">{taskDisplayTitle(task.title)}</span>
      {task.activePrimaryRun && <span className="task-running-spinner" role="status" aria-label={t(task.activePrimaryRun.state === "disconnected" ? "history.disconnected" : "history.running")} data-tooltip={t(task.activePrimaryRun.state === "disconnected" ? "history.disconnected" : "history.running")} />}
    </button>
    <div className="history-row-actions">
      {onRename && <button ref={renameButtonRef(task.taskId)} type="button" data-tooltip={t("nav.rename")} aria-label={t("nav.rename")} disabled={saving} onClick={() => beginRename(task)}>
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m12 3 5 5M3 17l4-1L17 6l-3-3L4 13l-1 4Z" /></svg>
      </button>}
      <button type="button" data-tooltip={isPinned ? t("nav.unpin") : t("nav.pin")} aria-label={isPinned ? t("nav.unpin") : t("nav.pin")}
        aria-pressed={isPinned} disabled={preferencesBusy} onClick={() => update(task, isPinned ? null : "pinned")}>
        <svg className="history-pin-icon" viewBox="0 0 24 24" aria-hidden="true">
          {isPinned
            ? <path d="M9 3h6M10 3v3M14 3v5l4 4v1M8.5 9.5 6 12v3h9M12 15v7M3 3l18 18" />
            : <path d="M9 3h6M10 3v5l-4 4v3h12v-3l-4-4V3M12 15v7" />}
        </svg>
      </button>
      <button type="button" data-tooltip={t("nav.archive")} aria-label={t("nav.archive")} disabled={preferencesBusy} onClick={() => update(task, "archived")}>
        <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 7v9h12V7M3 3h14v4H3ZM8 10h4" /></svg>
      </button>
    </div>
  </div>;
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [collapsedProjects, setCollapsedProjects] = useState<Set<string>>(() => new Set());
  const projectListsId = useId();
  return <div className="conversation-list"
    onDragOver={event => { setDropTarget(null); if (dragging.current) event.dataTransfer.dropEffect = "none"; }}
    onDragLeave={event => { if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setDropTarget(null); }}
    onDrop={finishDrag}>
    {error && <p className="history-error" role="alert">{error}</p>}
    {sharedHistory?.error && <div className="history-error" role="alert"><p>{sharedHistory.error}</p><button type="button" disabled={sharedHistory.busy} onClick={() => void retrySharedHistoryPreferences()}>{t("history.retrySettings")}</button></div>}
    {pinned.length > 0 && <HistorySection className="history-pinned" label={t("nav.pinnedSection")} {...sectionProps("pinned")}>
      {pinned.map(task => row(task, true))}
    </HistorySection>}
    <HistorySection className="history-projects" label={t("nav.projectsSection")} {...sectionProps("projects")} onCreate={onCreateProject}>
    {groups.map(group => {
      const isExpanded = expanded.has(group.key);
      const collapsed = collapsedProjects.has(group.key);
      const listId = `${projectListsId}-${group.key}`;
      // Keep the selected conversation and ongoing work visible even when the
      // rest of a long project's history is collapsed.
      const visible = isExpanded ? group.tasks : group.tasks.filter((task, index) =>
        index < PREVIEW_COUNT || task.taskId === selectedTaskId || task.activePrimaryRun);
      const hiddenCount = group.tasks.length - visible.length;
      const label = projectDisplayName(group.displayName);
      return <section className="project-group" key={group.key} aria-label={label} data-drop-edge={dropEdge("projects", group.key)} {...dragProps("projects", group.key, groups.map(item => item.key), "projects")} draggable={false}>
        <ProjectHoverCard project={projects.find(project => project.projectId === group.key)!} onEdit={onEditProject}
          conversationList={{ id: listId, expanded: !collapsed, onToggle: () => setCollapsedProjects(current => {
            const next = new Set(current);
            if (next.has(group.key)) next.delete(group.key); else next.add(group.key);
            return next;
          }) }}
          preferencesDisabled={preferencesBusy} pinned={projectPreferences[group.key] === "pinned"} onPin={() => updateProjectPreference(group.key, projectPreferences[group.key] === "pinned" ? null : "pinned")}
          onRemove={() => updateProjectPreference(group.key, "removed")} onReveal={onRevealProject}
          onCreateConversation={onCreateProjectConversation} createDisabled={createConversationDisabled} dragProps={dragProps("projects", group.key, groups.map(item => item.key), "projects")} />
        <div id={listId} className="project-conversations" hidden={collapsed}>
        {visible.map(task => row(task, false, `project:${group.key}`, group.tasks.map(item => item.taskId)))}
        {group.tasks.length === 0 && <p className="history-empty">{t("history.empty")}</p>}
        {(hiddenCount > 0 || isExpanded && group.tasks.length > PREVIEW_COUNT) && <button type="button"
          className="history-show-more" aria-expanded={isExpanded}
          onClick={() => setExpanded(current => {
            const next = new Set(current);
            if (next.has(group.key)) next.delete(group.key); else next.add(group.key);
            return next;
          })}>{t(isExpanded ? "history.showLess" : "history.showMore")}</button>}
        </div>
      </section>;
    })}
    </HistorySection>
    {removedProjects.length > 0 && <details className="removed-projects"><summary>{t("history.removedProjects", { count: removedProjects.length })}</summary>
      {removedProjects.map(project => <button key={project.projectId} type="button" disabled={preferencesBusy} onClick={() => updateProjectPreference(project.projectId, null)}>{t("history.restoreProject", { name: project.displayName })}</button>)}
    </details>}
    {archived.length > 0 && <HistorySection className="history-archived" label={t("nav.archivedSection")} {...sectionProps("archived")}>
      {archived.map(task => renaming === task.taskId ? renameForm(task) : <div className="history-archived-row" key={task.taskId} data-drop-edge={dropEdge("archived", task.taskId)} {...dragProps("archived", task.taskId, archived.map(item => item.taskId), "archived")}>
        <HistoryAgent task={task} />
        <span className="task-title" data-tooltip={taskDisplayTitle(task.title)} data-tooltip-overflow="">{taskDisplayTitle(task.title)}</span>
        <div className="history-row-actions">
          {onDelete && <DeleteConversation task={task} blocked={deletionBlocked(task)} onDelete={onDelete} />}
          {onRename && <button ref={renameButtonRef(task.taskId)} type="button" data-tooltip={t("nav.rename")} aria-label={t("nav.rename")} disabled={saving} onClick={() => beginRename(task)}>
            <svg viewBox="0 0 20 20" aria-hidden="true"><path d="m12 3 5 5M3 17l4-1L17 6l-3-3L4 13l-1 4Z" /></svg>
          </button>}
          <button type="button" data-tooltip={t("history.restoreTooltip")} aria-label={t("history.restoreTask", { title: taskDisplayTitle(task.title) })} disabled={preferencesBusy} onClick={() => update(task, null)}>
            <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 11v5h12v-5M10 13V3m-4 4 4-4 4 4" /></svg>
          </button>
        </div>
      </div>)}
    </HistorySection>}
  </div>;
}
