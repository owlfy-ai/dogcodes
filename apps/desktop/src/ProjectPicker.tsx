import { useEffect, useRef, useState } from "react";
import type { ProjectId, ProjectSummary } from "@dogcodes/protocol";
import { FloatingPopover } from "./FloatingPopover";
import { DropdownChevron } from "./DropdownChevron";
import { homeRelativePath, useHomeDirectory } from "./homeRelativePath";
import { FolderIcon } from "./ProjectHoverCard";

export function projectOptions(projects: ProjectSummary[], query: string, home: string | null = null) {
  const search = query.trim().toLocaleLowerCase();
  return projects.filter(project => project.displayName !== "快速对话"
    && `${project.displayName} ${[project.path, ...project.additionalDirectories].flatMap(path => [path, homeRelativePath(path, home)]).join(" ")}`.toLocaleLowerCase().includes(search));
}

export function ProjectPicker({ projects, selectedProject, disabled, onSelect, onOpenFolder }: {
  projects: ProjectSummary[];
  selectedProject: ProjectSummary | null;
  disabled: boolean;
  onSelect: (id: ProjectId) => void;
  onOpenFolder: () => void;
}) {
  const home = useHomeDirectory();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const options = projectOptions(projects, query, home);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const scratch = projects.find(project => project.displayName === "快速对话");
  const noProject = !selectedProject || selectedProject.projectId === scratch?.projectId;
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);
  const choose = (id: ProjectId) => {
    setOpen(false);
    trigger.current?.focus();
    onSelect(id);
  };
  return <div className="composer-project project-picker" ref={root}
    onBlur={event => {
      // WKWebView/Safari does not focus buttons on pointer clicks. In that case
      // relatedTarget is null: unmounting here would swallow the ensuing click.
      // Pointer exits are handled by the document pointerdown listener instead.
      if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
    }}
    onKeyDown={event => {
      if (event.key === "Escape") { event.preventDefault(); setOpen(false); trigger.current?.focus(); }
    }}>
    <button className="project-picker-trigger" type="button" ref={trigger} disabled={disabled}
      data-tooltip={selectedProject ? [selectedProject.path, ...selectedProject.additionalDirectories].join("\n") : undefined} aria-label="选择项目目录" aria-expanded={open} aria-haspopup="dialog"
      onClick={() => { setQuery(""); setOpen(!open); }}>
      <FolderIcon /><b>{noProject ? "无项目" : selectedProject.displayName}</b>{!!selectedProject?.additionalDirectories?.length && <small>{selectedProject.additionalDirectories.length + 1} 个目录</small>}<DropdownChevron />
    </button>
    {open && <FloatingPopover align="start" className="project-picker-menu" role="dialog" aria-label="选择工作区">
      <input autoFocus aria-label="搜索工作区" placeholder="搜索工作区" value={query}
        onChange={event => setQuery(event.target.value)} onKeyDown={event => {
          if (event.key === "Enter") event.preventDefault();
          if (event.key === "ArrowDown") { event.preventDefault(); root.current?.querySelector<HTMLButtonElement>(".project-picker-list button")?.focus(); }
        }} />
      <div className="project-picker-list">
        {options.map(project => <button type="button" key={project.projectId}
          data-tooltip={[project.path, ...project.additionalDirectories].join("\n")} aria-pressed={project.projectId === selectedProject?.projectId} onClick={() => choose(project.projectId)}>
          <FolderIcon /><span className="project-picker-copy">{project.displayName}<small>{homeRelativePath(project.path, home)}</small></span>
          {project.projectId === selectedProject?.projectId && <span aria-hidden="true">✓</span>}
        </button>)}
        {options.length === 0 && <p className="project-picker-empty">{query ? "没有匹配的工作区" : "尚未连接项目"}</p>}
      </div>
      <div className="project-picker-actions">
        <button type="button" onClick={() => { setOpen(false); onOpenFolder(); }}><svg className="workspace-folder" viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M10 4v12M4 10h12" /></svg>创建项目…</button>
        <button type="button" disabled={!scratch} aria-pressed={noProject} onClick={() => scratch && choose(scratch.projectId)}><svg className="workspace-folder" viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="M17 9.5a7 7 0 0 1-7 7H3l1-3A7 7 0 1 1 17 9.5Z" /></svg>不在项目中工作{noProject && <span aria-hidden="true">✓</span>}</button>
      </div>
    </FloatingPopover>}
  </div>;
}
