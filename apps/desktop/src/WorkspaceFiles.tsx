import { watchWorkspaceDirectory } from "./workspaceWatch";
import { TreeChevron } from "./TreeChevron";
import { ReviewFileLoading } from "./ReviewFileLoading";
import { LoadingStatus } from "./LoadingStatus";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { isWorkspaceNoise } from "./workspaceNoise";
import { WorkspaceFileFilter } from "./WorkspaceFileFilter";
import { WorkspaceFilePreview } from "./WorkspaceFilePreview";
import { FileIcon } from "./FileIcon";
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { directoryName } from "./ReviewFileTree";

type Entry = { name: string; path: string; directory: boolean };
type Preview = { text: string | null; image: string | null };
type Selection = { root: number; path: string };
function Folder({ taskId, root, path, name, selected, onSelect, collapseVersion, visible, reveal, hideNoise, scopeChanged }: { taskId: string; root: number; path: string; name: string; selected: Selection | null; onSelect: (selection: Selection) => void; collapseVersion: number; visible: boolean; hideNoise: boolean; scopeChanged: () => void; reveal?: { root: number; path: string; id: number } }) {
  const [open, setOpen] = useState(false);
  useEffect(() => { setOpen(false); }, [collapseVersion]);
  useEffect(() => { if (reveal?.root === root && (!path || reveal.path.startsWith(`${path}/`))) setOpen(true); }, [reveal, root, path]);
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [error, setError] = useState("");
  const [watchError, setWatchError] = useState("");
  useEffect(() => {
    if (!open || !visible) return;
    let active = true;
    let epoch = 0;
    const refresh = () => {
      const current = ++epoch;
      void invoke<Entry[]>("core_workspace_directory", { taskId, root, path }).then(value => {
        if (active && current === epoch) { setEntries(value); setError(""); }
      }).catch(cause => { if (active && current === epoch) setError(userFacingError(cause, "无法读取此目录，请稍后重试。")); });
    };
    const stop = watchWorkspaceDirectory({ taskId, root, path }, {
      refresh, error: message => { if (active) setWatchError(message); }, scopeChanged,
    });
    setWatchError("");
    return () => { active = false; stop(); };
  }, [taskId, root, path, open, visible, scopeChanged]);
  const shown = entries?.filter(entry => !hideNoise || !isWorkspaceNoise(entry.path));
  return <div className="review-directory">
    <button className="review-folder" type="button" aria-expanded={open} data-tooltip={path || name} onClick={() => setOpen(value => !value)}><TreeChevron expanded={open} /><span className="review-entry-name">{name}</span></button>
    <div className="review-tree-children" hidden={!open}>
      {watchError && <InlineError role="status">{watchError}</InlineError>}
      {error ? <InlineError>{error}</InlineError> : !shown ? <LoadingStatus>正在读取…</LoadingStatus> : shown.length === 0 ? <p className="review-tree-message">{entries?.length ? "没有可显示的文件" : "空目录"}</p> : shown.map(entry => entry.directory ? <Folder key={entry.path} {...{ taskId, root, selected, onSelect, collapseVersion, reveal, hideNoise, scopeChanged }} path={entry.path} name={entry.name} visible={visible && open} /> : <button key={entry.path} className={`workspace-file${selected?.root === root && selected.path === entry.path ? " selected" : ""}`} type="button" data-tooltip={entry.path} aria-pressed={selected?.root === root && selected.path === entry.path} ref={node => { if (node && reveal?.root === root && reveal.path === entry.path && selected?.root === root && selected.path === entry.path && visible && open) node.scrollIntoView?.({ block: "nearest" }); }} onClick={() => onSelect({ root, path: entry.path })}><FileIcon path={entry.path} /><span className="review-entry-name">{entry.name}</span></button>)}
    </div>
  </div>;
}
export function WorkspaceFiles({ taskId, visible = true, reveal, onEditProject }: { taskId: string; visible?: boolean; reveal?: { path: string; id: number }; onEditProject?: () => void }) {
  const [hideNoise, setHideNoise] = useState(() => {
    try { return localStorage.getItem("dogcodes.files.hideNoise") === "true"; } catch { return false; }
  });
  useEffect(() => {
    try { localStorage.setItem("dogcodes.files.hideNoise", String(hideNoise)); } catch { /* Storage is optional. */ }
  }, [hideNoise]);
  const [collapseVersion, setCollapseVersion] = useState(0);
  const [filter, setFilter] = useState("");
  const searchInput = useRef<HTMLInputElement>(null);
  const [rootsVersion, setRootsVersion] = useState(0);
  const scopeChanged = useCallback(() => { setRootsVersion(value => value + 1); setSelection(null); setRevealSelection(undefined); }, []);
  const [roots, setRoots] = useState<string[]>([]);
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [revealSelection, setRevealSelection] = useState<{ root: number; path: string; id: number }>();
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewError, setPreviewError] = useState("");
  useEffect(() => {
    let active = true;
    setLoaded(false); setError("");
    void invoke<string[]>("core_workspace_roots", { taskId }).then(value => { if (active) setRoots(value); }).catch(cause => { if (active) setError(userFacingError(cause, "无法读取工作区目录，请稍后重试。")); }).finally(() => { if (active) setLoaded(true); });
    return () => { active = false; };
  }, [taskId, rootsVersion]);
  useEffect(() => {
    let active = true;
    setPreview(null); setPreviewError("");
    if (selection) void invoke<Preview>("core_workspace_preview", { taskId, ...selection }).then(value => { if (active) setPreview(value); }).catch(cause => { if (active) setPreviewError(userFacingError(cause, "无法预览此文件，请稍后重试。")); });
    return () => { active = false; };
  }, [taskId, selection]);
  useEffect(() => {
    if (!reveal || !loaded) return;
    const normalize = (path: string) => path.replaceAll("\\", "/").replace(/\/+$/, "");
    const target = normalize(reveal.path);
    const matches = roots.map((path, root) => ({ path: normalize(path), root })).filter(item => target.startsWith(`${item.path}/`)).sort((a, b) => b.path.length - a.path.length);
    const match = matches[0];
    if (!match) { setError("此文件不在当前项目的文件树范围内"); return; }
    const selected = { root: match.root, path: target.slice(match.path.length + 1), id: reveal.id };
    if (isWorkspaceNoise(selected.path)) setHideNoise(false);
    setFilter(""); setSelection({ root: selected.root, path: selected.path }); setRevealSelection(selected);
  }, [reveal, roots, loaded]);
  const filtering = loaded && !error && roots.length > 0 && Boolean(filter.trim());
  return <div className={`review-body workspace-files${selection ? "" : " review-list-only"}`}>
    <nav aria-label="项目文件树"><div className="workspace-file-search"><input ref={searchInput} type="search" disabled={!loaded || !roots.length} aria-label="筛选项目文件" placeholder="搜索文件名或路径…" data-tooltip="支持 *.md、src/**/*.ts、file?.txt 等通配符" value={filter} onChange={event => setFilter(event.target.value)} onKeyDown={event => { if (event.key === "Escape") setFilter(""); }} /><button type="button" className="workspace-collapse-all" aria-label="全部折叠" data-tooltip="全部折叠（Collapse All）" onClick={() => setCollapseVersion(value => value + 1)}><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" aria-hidden="true"><path d="M5 2h9v9M2 5h9v9H2zM4 9.5h5" /></svg></button><button type="button" className="workspace-noise-toggle" aria-label="过滤干扰文件" aria-pressed={hideNoise} data-tooltip={hideNoise ? "显示全部文件" : "隐藏依赖、构建产物、开发工具及隐藏文件"} onClick={() => {
      setHideNoise(value => !value);
      if (!hideNoise && selection && isWorkspaceNoise(selection.path)) { setSelection(null); setRevealSelection(undefined); }
    }}><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" aria-hidden="true"><path d="M2 3h12L9.5 8v4.5l-3 1V8L2 3Z" /></svg></button></div>{filtering && <WorkspaceFileFilter taskId={taskId} query={filter.trim()} roots={roots} selected={selection} onSelect={setSelection} visible={visible} collapseVersion={collapseVersion} hideNoise={hideNoise} onClear={() => { setFilter(""); searchInput.current?.focus(); }} onShowAll={() => setHideNoise(false)} />}<div className={`workspace-directory-list${loaded && !error && !roots.length ? " workspace-directory-list--empty" : ""}`} hidden={filtering}>{error ? <p role="alert">{error}</p> : !loaded ? <LoadingStatus layout="panel">正在读取…</LoadingStatus> : roots.length === 0 ? <ReviewEmptyState kind="folder" title="尚未关联项目目录" description={onEditProject ? "为项目添加目录后，即可浏览文件。" : "在左侧编辑项目并添加目录，即可浏览文件。"} action={onEditProject && <button type="button" className="review-empty-action" onClick={onEditProject}>添加项目目录</button>} /> : roots.map((path, root) => <Folder key={`${root}:${path}`} taskId={taskId} root={root} path="" name={directoryName(path)} selected={selection} onSelect={setSelection} collapseVersion={collapseVersion} reveal={revealSelection} hideNoise={hideNoise} scopeChanged={scopeChanged} visible={visible && !filter.trim()} />)}</div></nav>
    {selection && <div className="review-file-body"><><div className="review-file-heading"><strong data-tooltip={`${roots[selection.root]}/${selection.path}`}>{directoryName(roots[selection.root] ?? "")}/{selection.path}</strong></div>{previewError ? <InlineError>{previewError}</InlineError> : !preview ? <ReviewFileLoading /> : <WorkspaceFilePreview preview={preview} path={selection.path} />}</></div>}
  </div>;
}
