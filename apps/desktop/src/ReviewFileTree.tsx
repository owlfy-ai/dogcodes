import { TreeChevron } from "./TreeChevron";
import { reviewFileKey } from "./useReviewFileSelection";
import { FileIcon } from "./FileIcon";
import { useMemo, useState } from "react";
import type { ReviewChange, RootChanges } from "./reviewClient";

interface Directory { name: string; path: string; directories: Map<string, Directory>; files: ReviewChange[] }
export function buildReviewTree(files: ReviewChange[]): Directory {
  const root: Directory = { name: "", path: "", directories: new Map(), files: [] };
  for (const file of files) {
    let node = root;
    const segments = file.path.split("/");
    for (const segment of segments.slice(0, -1)) {
      let child = node.directories.get(segment);
      if (!child) {
        child = { name: segment, path: node.path ? `${node.path}/${segment}` : segment, directories: new Map(), files: [] };
        node.directories.set(segment, child);
      }
      node = child;
    }
    node.files.push(file);
  }
  return root;
}

export const directoryName = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).at(-1) || path;
export function ReviewFileTree({ result, filter, selectedPath, busy, onSelect, active, onActivate, onContextMenu, view = "tree", showRoot = false, selectedFiles }: {
  selectedFiles?: Set<string>;
  onContextMenu?: (file: ReviewChange, point: { x: number; y: number; trigger: HTMLButtonElement }) => void;
  view?: "list" | "tree"; showRoot?: boolean; result: RootChanges; filter: string; selectedPath?: string; busy: boolean; onSelect: (path: string) => void; active?: boolean; onActivate?: () => void;
}) {
  const [folded, setFolded] = useState<Set<string>>(() => new Set());
  const files = useMemo(() => result.files.filter(f => f.path.toLowerCase().includes(filter.toLowerCase()) || f.previousPath?.toLowerCase().includes(filter.toLowerCase())), [result.files, filter]);
  const tree = useMemo(() => view === "tree" ? buildReviewTree(files) : { name: "", path: "", directories: new Map<string, Directory>(), files }, [files, view]);
  const toggle = (key: string) => setFolded(old => { const next = new Set(old); if (next.has(key)) next.delete(key); else next.add(key); return next; });
  const counts = Object.values(result.statistics);
  const open = view === "list" || Boolean(filter) || !folded.has("root");
  const renderDirectory = (node: Directory): React.ReactNode => <>
    {[...node.directories.values()].sort((a, b) => a.name.localeCompare(b.name)).map(initial => {
      let child = initial;
      let name = child.name;
      while (!child.files.length && child.directories.size === 1) { child = [...child.directories.values()][0]; name += ` / ${child.name}`; }
      const expanded = Boolean(filter) || !folded.has(initial.path);
      return <div className="review-directory" key={initial.path}>
        <button type="button" className="review-folder" aria-expanded={expanded} onClick={() => toggle(initial.path)} data-tooltip={initial.path}><TreeChevron expanded={expanded} /><span className="review-entry-name">{name}</span></button>
        {expanded && <div className="review-tree-children">{renderDirectory(child)}</div>}
      </div>;
    })}
    {[...node.files].sort((a, b) => a.path.localeCompare(b.path)).map(file => {
      const status = file.previousPath || file.kind === "renamed" ? "R" : result.untracked.includes(file.path) ? "U" : ({ added: "A", deleted: "D", modified: "M", renamed: "R" } as const)[file.kind];
      const label = { M: "修改", U: "未跟踪", A: "新增", D: "删除", R: "重命名" }[status];
      const stats = result.statistics[file.path];
      return <button type="button" className="review-file-entry" data-review-key={reviewFileKey(result.root.id, file.path)} aria-pressed={selectedFiles ? selectedFiles.has(reviewFileKey(result.root.id, file.path)) : undefined} disabled={busy} key={file.path} data-tooltip={file.previousPath ? `${file.previousPath} → ${file.path}` : file.path} aria-label={file.path} aria-current={selectedPath === file.path ? "true" : undefined} onClick={() => onSelect(file.path)} onContextMenu={event => { if (!onContextMenu) return; event.preventDefault(); onContextMenu(file, { x: event.clientX, y: event.clientY, trigger: event.currentTarget }); }} onKeyDown={event => { if (!onContextMenu || !(event.key === "ContextMenu" || event.key === "F10" && event.shiftKey)) return; event.preventDefault(); const rect = event.currentTarget.getBoundingClientRect(); onContextMenu(file, { x: rect.left + 12, y: rect.bottom, trigger: event.currentTarget }); }}>
        <FileIcon path={file.path} /><span className="review-entry-name">{directoryName(file.path)}</span>
        <span className="review-entry-counts">{stats?.additions != null && <><span className="review-add-count">+{stats.additions}</span> <span className="review-delete-count">−{stats.deletions}</span></>}{stats?.error && <span data-tooltip="行数统计暂不可用">…</span>}</span>
        <span className={`review-status status-${status}`} data-tooltip={label} aria-label={label}>{status}</span>
      </button>;
    })}
  </>;
  return <section className="review-root" data-view={view} data-root-id={result.root.id} data-active={active || undefined}>
    {showRoot && view === "list" && <div className="review-root-label" data-tooltip={result.root.path}>{directoryName(result.root.path)}{!result.root.git && " · 会话变更"}</div>}
    {view === "tree" && <button type="button" className="review-root-heading" aria-label={`目录 ${directoryName(result.root.path)}`} aria-expanded={open} onClick={() => { toggle("root"); onActivate?.(); }}>
      <TreeChevron expanded={open} /><span className="review-entry-name">{directoryName(result.root.path)}{!result.root.git && " · 会话变更"}</span>
      <span className="review-entry-counts"><span className="review-add-count">+{counts.reduce((n, s) => n + (s.additions ?? 0), 0)}</span> <span className="review-delete-count">−{counts.reduce((n, s) => n + (s.deletions ?? 0), 0)}</span></span>
      {(result.error || counts.some(s => s.error)) && <span data-tooltip="部分统计不可用">!</span>}
    </button>}
    {open && <div className="review-root-content">
      {Boolean(result.excludedRoots?.length) && <details className="review-scope-note"><summary>已排除 {result.excludedRoots!.length} 个子仓库目录</summary><p>独立仓库不计入父目录差异，请在对应目录查看。</p><ul>{result.excludedRoots!.map(path => <li key={path}>{directoryName(path)}</li>)}</ul></details>}
      {result.error ? <p className="review-tree-message" role="alert" data-tooltip="请刷新后重试">无法读取此目录的差异</p> : result.files.length === 0 ? <p className="review-tree-message">没有变更</p> : !tree.files.length && !tree.directories.size ? <p className="review-tree-message">没有匹配文件</p> : renderDirectory(tree)}
    </div>}
  </section>;
}
