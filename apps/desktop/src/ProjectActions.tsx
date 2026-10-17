import { useEffect, useId, useRef, useState } from "react";
import type { ProjectSummary } from "@dogcodes/protocol";
import { FloatingPopover } from "./FloatingPopover";
import { userFacingError } from "./userFacingError";

export interface ProjectActionsProps {
  project: ProjectSummary;
  pinned?: boolean;
  onPin?: () => void;
  onEdit?: (project: ProjectSummary) => void;
  onReveal?: (project: ProjectSummary) => Promise<void>;
  onRemove?: () => void;
  onCreateConversation?: (project: ProjectSummary) => void;
  createDisabled?: boolean;
  preferencesDisabled?: boolean;
}

export function ProjectActions({ project, pinned, onPin, onEdit, onReveal, onRemove, onCreateConversation, createDisabled, preferencesDisabled }: ProjectActionsProps) {
  const [open, setOpen] = useState(false);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const controls = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const id = useId();
  const close = () => { setOpen(false); trigger.current?.focus(); };
  useEffect(() => {
    if (!open) return;
    controls.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    const outside = (event: PointerEvent) => { if (!controls.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  const action = (run: () => void) => { close(); run(); };
  return <div className="project-actions" ref={controls} onBlur={event => {
    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  }}>
    <button ref={trigger} className="project-action" type="button" data-tooltip="项目操作" aria-label={`${project.displayName} 项目操作`} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined} onClick={() => { setError(""); setOpen(value => !value); }}>⋯</button>
    {onCreateConversation && <button className="project-action" type="button" data-tooltip="新建会话" aria-label={`在 ${project.displayName} 中新建会话`} disabled={createDisabled} onClick={() => { setOpen(false); onCreateConversation(project); }}>＋</button>}
    {open && <FloatingPopover id={id} placement="bottom" className="project-action-menu" role="menu" aria-label={`${project.displayName} 项目操作`} onKeyDown={event => {
      const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
      if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault(); event.stopPropagation();
        items[event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
      }
    }}>
      {onPin && <button type="button" role="menuitem" disabled={preferencesDisabled} onClick={() => action(onPin)}><ActionIcon kind="pin" />{pinned ? "取消置顶" : "置顶项目"}</button>}
      {onEdit && <button type="button" role="menuitem" onClick={() => action(() => onEdit(project))}><ActionIcon kind="edit" />编辑项目</button>}
      {onReveal && <><hr /><button type="button" role="menuitem" disabled={busy} onClick={async () => {
        setBusy(true); setError("");
        try { await onReveal(project); close(); }
        catch (cause) { setError(userFacingError(cause, "无法在文件管理器中查看项目，请重试。")); }
        finally { setBusy(false); }
      }}><ActionIcon kind="folder" />{busy ? "正在打开…" : "在文件管理器中查看"}</button></>}
      {onRemove && <><hr /><button type="button" role="menuitem" disabled={preferencesDisabled} onClick={() => action(onRemove)}><ActionIcon kind="remove" />移除项目</button><small>保留会话和本地文件</small></>}
      {error && <p role="alert">{error}</p>}
    </FloatingPopover>}
  </div>;
}

function ActionIcon({ kind }: { kind: "pin" | "edit" | "folder" | "remove" }) {
  const paths = { pin: "M7 3h6M8 3v4l-4 4v2h12v-2l-4-4V3M10 13v5", edit: "m12 3 5 5M3 17l4-1L17 6l-3-3L4 13l-1 4Z", folder: "M2 6V4h6l2 2h7v10H2V6Z", remove: "m5 5 10 10M15 5 5 15" };
  return <svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d={paths[kind]} /></svg>;
}
