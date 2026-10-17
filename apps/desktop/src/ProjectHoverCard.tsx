import { useEffect, useId, useRef, useState, type HTMLAttributes } from "react";
import { homeRelativePath, useHomeDirectory } from "./homeRelativePath";
import { FloatingPopover } from "./FloatingPopover";
import "./projectHoverCard.css";
import { ProjectActions, type ProjectActionsProps } from "./ProjectActions";

export function FolderIcon({ open = true }: { open?: boolean }) {
  return <svg className="workspace-folder" viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d={open
    ? "M4 16a1.5 1.5 0 0 1-1.48-1.25L1.5 6a1.5 1.5 0 0 1 1.5-1.75h4.25l2 2H15a1.5 1.5 0 0 1 1.5 1.5V9M4 16l2.05-5.97A1.5 1.5 0 0 1 7.47 9H17.5a1 1 0 0 1 .94 1.34l-1.7 4.67A1.5 1.5 0 0 1 15.33 16H4Z"
    : "M2.5 5.5A1.5 1.5 0 0 1 4 4h4l2 2h6a1.5 1.5 0 0 1 1.5 1.5v7A1.5 1.5 0 0 1 16 16H4a1.5 1.5 0 0 1-1.5-1.5V5.5Z"} /></svg>;
}

export function ProjectHoverCard({ project, onEdit, dragProps, conversationList, ...actions }: ProjectActionsProps & {
  dragProps?: HTMLAttributes<HTMLElement>;
  conversationList?: { id: string; expanded: boolean; onToggle: () => void };
}) {
  const [open, setOpen] = useState(false);
  const home = useHomeDirectory();
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const trigger = useRef<HTMLButtonElement>(null);
  const id = useId();
  const label = project.displayName === "快速对话" ? "无项目" : project.displayName;
  const directories = [project.path, ...project.additionalDirectories];
  const cancelTimer = () => clearTimeout(timer.current);
  const show = () => { cancelTimer(); setOpen(true); };
  const close = () => { cancelTimer(); setOpen(false); };
  useEffect(() => () => clearTimeout(timer.current), []);
  return <header className="project-hover-anchor" data-info-open={open || undefined}
    onMouseEnter={() => { cancelTimer(); timer.current = setTimeout(() => setOpen(true), 280); }}
    onMouseLeave={() => { cancelTimer(); timer.current = setTimeout(() => setOpen(false), 180); }}
    onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) close(); }}
    onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); trigger.current?.focus(); close(); } }}>
    <button {...dragProps} onDragStart={event => { close(); dragProps?.onDragStart?.(event); }} ref={trigger} className="project-heading-trigger" type="button" aria-expanded={conversationList?.expanded ?? open} aria-controls={conversationList?.id ?? (open ? id : undefined)} onFocus={show}
      onClick={() => { if (conversationList) { close(); conversationList.onToggle(); } else show(); }}>
      <FolderIcon open={conversationList?.expanded ?? true} /><span className="workspace-copy"><b>{label}</b></span>{actions.pinned && <svg className="project-pin-mark" viewBox="0 0 20 20" fill="none" role="img" aria-label="项目已置顶"><path d="M7 3h6M8 3v4l-4 4v2h12v-2l-4-4V3M10 13v5" /></svg>}
      {conversationList && <svg className="project-list-chevron" viewBox="0 0 16 16" aria-hidden="true" style={{ transform: conversationList.expanded ? undefined : "rotate(-90deg)" }}><path d="m4 6 4 4 4-4" /></svg>}
    </button>
    <div className="project-actions-anchor" onMouseEnter={close} onFocus={close}><ProjectActions project={project} onEdit={project.displayName === "快速对话" ? undefined : onEdit} {...actions} /></div>
    {open && <FloatingPopover placement="right" id={id} className="project-hover-card" role="region" aria-label={`${label} 项目信息`} onMouseEnter={cancelTimer}>
      <div className="project-hover-title"><FolderIcon /><strong>{label}</strong></div>
      <div className="project-hover-count"><svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M17 9.5a7 7 0 0 1-7 7H3l1-3A7 7 0 1 1 17 9.5Z" /></svg>{project.taskCount} 个会话</div>
      <ul className="project-hover-directories">{directories.map((directory, index) => <li key={directory}><FolderIcon /><span data-tooltip={directory}>{homeRelativePath(directory, home)}</span>{directories.length > 1 && index === 0 && <small>主目录</small>}</li>)}</ul>
      {onEdit && project.displayName !== "快速对话" && <button className="project-hover-edit" type="button" onClick={() => { trigger.current?.focus(); close(); onEdit(project); }}>
        <svg viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="m12 3 5 5M3 17l4-1L17 6l-3-3L4 13l-1 4Z" /></svg>编辑项目
      </button>}
    </FloatingPopover>}
  </header>;
}
