import { userFacingError } from "./userFacingError";
import { Fragment, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FloatingPopover } from "./FloatingPopover";
import { LoadingStatus } from "./LoadingStatus";
import "./reviewWorkspaces.css";
import { translate as t } from "./i18n";

export interface ReviewWorkspace {
  taskId: string; name: string; conversationTitle: string; path: string;
  branch: string | null; head: string | null; changedFiles: number | null;
  current: boolean; primary: boolean; git: boolean; running: boolean; error: string | null;
}
export interface ReviewWorkspaceApi { list(taskId: string): Promise<ReviewWorkspace[]> }
export const reviewWorkspaceApi: ReviewWorkspaceApi = {
  list: taskId => invoke("core_review_workspaces", { taskId }),
};
export const workspaceBranch = (item: ReviewWorkspace) => !item.git ? t("workspaces.folder") : item.branch || (item.head ? t("workspaces.detachedHead", { head: item.head.slice(0, 8) }) : t("workspaces.branchUnknown"));
// Home-relative paths read faster; the full path stays in the tooltip.
const shortPath = (path: string) => path.replace(/^(\/Users|\/home)\/[^/]+(?=\/|$)/, "~");
const changeState = (item: ReviewWorkspace) => !item.git || item.changedFiles === null ? "unknown" : item.changedFiles === 0 ? "clean" : "dirty";
const changes = (item: ReviewWorkspace) => !item.git ? t("workspaces.noGit") : item.changedFiles === null ? t("workspaces.statusUnknown") : item.changedFiles === 0 ? t("workspaces.clean") : t("workspaces.changedFiles", { count: item.changedFiles });

export function ReviewWorkspaceBrowser({ taskId, running, visible = true, fileRequestId, requestedPath, api = reviewWorkspaceApi, onOpenConversation, children }: {
  taskId: string; running: boolean; visible?: boolean; fileRequestId?: number; requestedPath?: string;
  api?: ReviewWorkspaceApi; onOpenConversation?: (taskId: string) => void;
  children: (target: { taskId: string; running: boolean; visible: boolean; current: boolean; header: ReactNode; onBusy: (busy: boolean) => void; onChanged: () => void }) => ReactNode;
}) {
  const [items, setItems] = useState<ReviewWorkspace[]>([]);
  const [viewed, setViewed] = useState(taskId);
  const [visited, setVisited] = useState([taskId]);
  const [operations, setOperations] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(true), [error, setError] = useState("");
  const [revision, setRevision] = useState(0), [open, setOpen] = useState(false);
  const host = useRef<HTMLDivElement>(null), trigger = useRef<HTMLButtonElement>(null);
  const focusAfterSwitch = useRef(false);
  const refresh = useCallback(() => setRevision(value => value + 1), []);
  const current = items.find(item => item.current);
  const target = items.find(item => item.taskId === viewed) ?? current;
  const selectedId = target?.taskId ?? taskId;
  const locked = Boolean(operations[selectedId]);
  useEffect(() => {
    if (focusAfterSwitch.current) { focusAfterSwitch.current = false; trigger.current?.focus(); }
  }, [selectedId]);
  useEffect(() => {
    if (!visible) return;
    let active = true, pending = false;
    const load = async () => {
      if (pending) return;
      pending = true; setLoading(true);
      try {
        const next = await api.list(taskId);
        if (!next.some(item => item.current && item.taskId === taskId)) throw new Error(t("workspaces.currentUnavailable"));
        if (active) {
          setItems(next); setError("");
          setViewed(previous => next.some(item => item.taskId === previous) ? previous : taskId);
        }
      } catch (cause) { if (active) setError(userFacingError(cause, t("workspaces.listFailed"))); }
      finally { pending = false; if (active) setLoading(false); }
    };
    void load();
    const timer = window.setInterval(() => { if (document.visibilityState !== "hidden") void load(); }, 15000);
    const focus = () => void load();
    window.addEventListener("focus", focus);
    return () => { active = false; clearInterval(timer); window.removeEventListener("focus", focus); };
  }, [api, taskId, visible, revision, running]);
  useEffect(() => { if (!visible) setOpen(false); }, [visible]);
  // A file opened from the chat always belongs to that chat's workspace.
  const lastRequest = useRef({ requestedPath, fileRequestId });
  useEffect(() => {
    const previous = lastRequest.current;
    lastRequest.current = { requestedPath, fileRequestId };
    if (requestedPath && (previous.requestedPath !== requestedPath || previous.fileRequestId !== fileRequestId)) {
      setViewed(taskId); setOpen(false);
    }
  }, [taskId, requestedPath, fileRequestId]);
  const close = () => { setOpen(false); trigger.current?.focus(); };
  useEffect(() => {
    if (!open) return;
    host.current?.querySelector<HTMLButtonElement>('[role="menuitemradio"][aria-checked="true"]')?.focus();
    const outside = (event: PointerEvent) => { if (!host.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.preventDefault(); close(); } };
    document.addEventListener("pointerdown", outside); document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open]);
  const choose = (id: string) => {
    if (locked) return;
    setVisited(previous => previous.includes(id) ? previous : [...previous, id]);
    focusAfterSwitch.current = id !== selectedId;
    setViewed(id); close();
  };
  const header = <div className="review-workspace" ref={host}>
    <div className="review-workspace-heading">
      <button ref={trigger} type="button" className="review-workspace-trigger" aria-label={t("workspaces.switchLabel")} aria-haspopup="menu" aria-expanded={open}
        disabled={locked || !items.length} data-tooltip={t("workspaces.switchTooltip")}
        onClick={() => setOpen(value => !value)} onKeyDown={event => { if (["ArrowDown", "ArrowUp"].includes(event.key)) { event.preventDefault(); setOpen(true); } }}>
        <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><rect x="2" y="3" width="16" height="14" rx="2"/><path d="M7 3v14M10 7h5m-5 3h5"/></svg>
        <span className="review-workspace-name" data-tooltip={target?.name} data-tooltip-overflow="">{target?.name ?? t("terminalDock.workspace")}</span>
        <svg className="review-workspace-caret" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m4.5 6.5 3.5 3.5 3.5-3.5" /></svg>
        <span className="review-workspace-badge" data-current={selectedId === taskId || undefined}>{t(selectedId === taskId ? "workspaces.usedByCurrent" : "workspaces.viewing")}</span>
      </button>
      <button type="button" className="review-icon-button review-workspace-refresh" aria-label={t("workspaces.refresh")} data-tooltip={t("workspaces.refresh")} disabled={loading || locked} onClick={refresh}>
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3" /></svg>
      </button>
    </div>
    {target && <>
      <div className="review-workspace-meta">
        <span className="review-workspace-branch" data-tooltip={workspaceBranch(target)}>
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.6" /><circle cx="4.5" cy="12.5" r="1.6" /><circle cx="11.5" cy="5" r="1.6" /><path d="M4.5 5.1v5.8M11.5 6.6c0 3-3 3.2-6.4 4.6" /></svg>
          <span>{workspaceBranch(target)}</span>
        </span>
        <span className="review-workspace-path" data-tooltip={target.path}>{shortPath(target.path)}</span>
        <span className="review-workspace-changes" data-state={changeState(target)} data-tooltip={t("workspaces.changesTooltip")}>{changes(target)}</span>
        {target.running && <span className="review-workspace-running">{t("workspaces.agentWorking")}</span>}
      </div>
      {target.error && <p className="review-workspace-error" role="status">{t("workspaces.statusFailed", { error: target.error })}</p>}
    </>}
    {selectedId !== taskId && <div className="review-workspace-context">
      <p>{t("workspaces.stillUsesBefore")}<strong>{current?.name ?? t("workspaces.originalWorkspace")}</strong>{t("workspaces.stillUsesAfter")}</p>
      <div><button type="button" disabled={locked} onClick={() => choose(taskId)}>{t("workspaces.backToCurrent")}</button>
      {onOpenConversation && <button type="button" disabled={locked} data-tooltip={target?.conversationTitle} onClick={() => onOpenConversation(selectedId)}>{t("workspaces.openConversation")}</button>}</div>
    </div>}
    {loading && !items.length && <LoadingStatus>{t("workspaces.loading")}</LoadingStatus>}
    {error && <p className="review-workspace-error" role="alert">{error}<button type="button" disabled={loading || locked} onClick={refresh}>{t("common.retry")}</button></p>}
    {open && <FloatingPopover className="review-workspace-menu" role="menu" aria-label={t("workspaces.list")} placement="bottom" align="start" onKeyDown={event => {
      const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)')];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "ArrowDown" ? (index + 1) % buttons.length : event.key === "ArrowUp" ? (index - 1 + buttons.length) % buttons.length : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : null;
      if (next !== null) { event.preventDefault(); buttons[next]?.focus(); }
      if (event.key === "Tab") setOpen(false);
    }}>
      <header><strong>{t("workspaces.switchTitle")}</strong><small>{t("workspaces.switchSubtitle")}</small></header>
      {items.map(item => <button type="button" role="menuitemradio" aria-checked={item.taskId === selectedId} key={item.taskId} disabled={locked} onClick={() => choose(item.taskId)}>
        <span className="review-workspace-option-title"><strong>{item.name}</strong><small>{item.current ? t("workspaces.usedByCurrent") : item.primary ? t("workspaces.primary") : "Worktree"}</small></span>
        <span className="review-workspace-meta"><span>{workspaceBranch(item)}</span><span>{changes(item)}</span>{item.running && <span>{t("workspaces.agentWorking")}</span>}</span>
        <span className="review-workspace-path" data-tooltip={item.path}>{shortPath(item.path)}</span>
      </button>)}
    </FloatingPopover>}
  </div>;
  return <>{visited.filter(id => id === taskId || items.some(item => item.taskId === id)).map(id => {
    const item = items.find(item => item.taskId === id);
    // Keep each reader directly in the app grid so the existing dock layout applies.
    return <Fragment key={id}>
      {children({ taskId: id, current: id === taskId, running: Boolean(item?.running || (id === taskId && running)), visible: visible && id === selectedId,
        header: id === selectedId ? header : null,
        onBusy: busy => setOperations(previous => previous[id] === busy ? previous : { ...previous, [id]: busy }), onChanged: refresh })}
    </Fragment>;
  })}</>;
}
