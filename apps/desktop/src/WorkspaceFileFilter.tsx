import { TreeChevron } from "./TreeChevron";
import { LoadingStatus } from "./LoadingStatus";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { useEffect, useState, useRef, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FileIcon } from "./FileIcon";
import { directoryName } from "./ReviewFileTree";
import { translate as t } from "./i18n";
type Match = { root: number; path: string };
type Tree = { folders: Map<string, Tree>; files: Match[] };
function build(files: Match[]): Tree {
  const tree: Tree = { folders: new Map(), files: [] };
  for (const file of files) {
    let node = tree;
    for (const name of file.path.split("/").slice(0, -1)) {
      if (!node.folders.has(name)) node.folders.set(name, { folders: new Map(), files: [] });
      node = node.folders.get(name)!;
    }
    node.files.push(file);
  }
  return tree;
}
function FilterDirectory({ label, tooltip, collapseVersion, children }: { label: string; tooltip?: string; collapseVersion: number; children: ReactNode }) {
  const [open, setOpen] = useState(true);
  const previous = useRef(collapseVersion);
  useEffect(() => {
    if (previous.current !== collapseVersion) { previous.current = collapseVersion; setOpen(false); }
  }, [collapseVersion]);
  return <details open={open}><summary data-tooltip={tooltip} onClick={event => { event.preventDefault(); setOpen(value => !value); }}><TreeChevron expanded={open} /><span className="review-entry-name">{label}</span></summary><div className="review-tree-children" hidden={!open}>{children}</div></details>;
}
function Matches({ tree, selected, onSelect, collapseVersion }: { collapseVersion: number; tree: Tree; selected: Match | null; onSelect: (file: Match) => void }) {
  return <>{[...tree.folders].sort(([a], [b]) => a.localeCompare(b)).map(([name, child]) => <FilterDirectory key={name} label={name} collapseVersion={collapseVersion}><Matches tree={child} selected={selected} onSelect={onSelect} collapseVersion={collapseVersion} /></FilterDirectory>)}{tree.files.map(file => <button type="button" key={file.path} className={`workspace-file${selected?.root === file.root && selected.path === file.path ? " selected" : ""}`} data-tooltip={file.path} onClick={() => onSelect(file)}><FileIcon path={file.path} /><span className="review-entry-name">{directoryName(file.path)}</span></button>)}</>;
}
export function WorkspaceFileFilter({ taskId, query, roots, selected, onSelect, visible, collapseVersion = 0, hideNoise = false, onClear, onShowAll }: { onClear?: () => void; onShowAll?: () => void; collapseVersion?: number; hideNoise?: boolean; taskId: string; query: string; roots: string[]; selected: Match | null; onSelect: (file: Match) => void; visible: boolean }) {
  const [result, setResult] = useState<{ files: Match[]; partial: boolean } | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    setResult(null); setError("");
    if (!visible) return;
    const timer = window.setTimeout(() => {
      void invoke<{ files: Match[]; partial: boolean }>("core_workspace_find_files", { taskId, query, hideNoise }).then(value => { if (active) setResult(value); }).catch(cause => { if (active) setError(userFacingError(cause, t("fileFilter.failed"))); });
    }, 300);
    return () => { active = false; window.clearTimeout(timer); };
  }, [taskId, query, visible, hideNoise]);
  return <div className="workspace-filter-results">{error ? <InlineError>{error}</InlineError> : !result ? <LoadingStatus layout="panel">{t("fileFilter.filtering")}</LoadingStatus> : <>{result.partial && <p role="status">{t("fileFilter.partial")}</p>}{!result.files.length ? <ReviewEmptyState kind="search" title={t("fileTree.noMatches")}
        description={t(result.partial ? "fileFilter.partialHint" : hideNoise ? "fileFilter.noiseHint" : "fileFilter.tryOther")}
        action={<>{onClear && <button type="button" className="review-empty-action" onClick={onClear}>{t("fileFilter.clear")}</button>}{hideNoise && onShowAll && <button type="button" className="review-empty-action review-empty-action--secondary" onClick={onShowAll}>{t("fileFilter.showAll")}</button>}</>} /> : roots.map((path, root) => { const files = result.files.filter(file => file.root === root); return files.length ? <FilterDirectory key={`${query}:${root}`} tooltip={path} label={directoryName(path)} collapseVersion={collapseVersion}><Matches tree={build(files)} selected={selected} onSelect={onSelect} collapseVersion={collapseVersion} /></FilterDirectory> : null; })}</>}</div>;
}
