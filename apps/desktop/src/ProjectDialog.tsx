import { userFacingError } from "./userFacingError";
import { focusDialogTitle } from "./dialogFocus";
import { type FormEvent, useEffect, useRef, useState } from "react";
import type { CoreClient, ProjectId, ProjectSummary } from "@dogcodes/protocol";
import { pickProjectDirectory } from "./localProject";
import { FolderIcon } from "./ProjectHoverCard";
import { useI18n } from "./i18n";

export function ProjectDialog({
  client,
  project,
  onClose,
  onReady,
}: {
  client: Pick<CoreClient, "registerProject" | "updateProject">;
  project?: ProjectSummary;
  onClose: () => void;
  onReady: (projectId: ProjectId) => Promise<void>;
}) {
  const { t } = useI18n();
  const [name, setName] = useState(project?.displayName ?? "");
  const [path, setPath] = useState("");
  const [directories, setDirectories] = useState<string[]>(() => project ? [project.path, ...project.additionalDirectories] : []);
  const [busy, setBusy] = useState(false);
  const [choosingFolder, setChoosingFolder] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const dialog = useRef<HTMLFormElement>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    focusDialogTitle(dialog.current);
    return () => previous?.focus();
  }, []);

  const start = async (event: FormEvent) => {
    event.preventDefault();
    if (busy || !name.trim() || directories.length === 0) return;
    setBusy(true);
    setMessage(null);
    try {
      if (project) {
        if (!client.updateProject) throw new Error("当前版本不支持编辑项目");
        await client.updateProject(project, name.trim(), directories[0], directories.slice(1));
        await onReady(project.projectId);
      } else {
        const projectId = await client.registerProject(name.trim(), directories[0], directories.slice(1));
        await onReady(projectId);
      }
    } catch (cause) {
      setMessage(userFacingError(cause, project ? "项目未能保存，请重试。" : "项目未能创建，请重试。"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget && !busy) onClose();
    }}>
      <form ref={dialog} className="project-dialog" role="dialog" aria-modal="true" aria-labelledby="project-dialog-title" onSubmit={start} onKeyDown={event => {
        if (event.key === "Escape" && !busy) { event.preventDefault(); onClose(); }
        if (event.key === "Tab") {
          const elements = [...(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)') ?? [])];
          const first = elements[0], last = elements[elements.length - 1];
          if (event.shiftKey && (document.activeElement === first || document.activeElement?.hasAttribute("data-dialog-title"))) { event.preventDefault(); last?.focus(); }
          if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }
      }}>
        <header><h2 id="project-dialog-title" tabIndex={-1} data-dialog-title>{project ? t("nav.editProject") : t("nav.createProject")}</h2><button type="button" aria-label={t("common.close")} disabled={busy} onClick={onClose}>×</button></header>
        <label>项目名称<input required maxLength={120} disabled={busy} placeholder="输入项目名称" value={name} onChange={(event) => setName(event.target.value)} /></label>
        <label>项目目录</label>
        <div className="project-directory-list">
          {directories.map((directory, index) => <div className="project-directory-row" key={directory}>
            <FolderIcon /><span data-tooltip={directory}>{directory}<small>{index === 0 ? "主目录" : "关联目录"}</small></span>
            {index > 0 && <button type="button" disabled={busy} aria-label={`将 ${directory} 设为主目录`} onClick={() => setDirectories([directory, ...directories.filter(d => d !== directory)])}>设为主目录</button>}
            <button type="button" disabled={busy} aria-label={`移除 ${directory}`} onClick={() => setDirectories(directories.filter(d => d !== directory))}>×</button>
          </div>)}
          {directories.length === 0 && <p>添加 Agent 可以读取和编辑的项目文件夹</p>}
        {"__TAURI_INTERNALS__" in window && <button type="button" disabled={busy || directories.length >= 32} onClick={async () => {
          setBusy(true);
          setChoosingFolder(true);
          setMessage(null);
          try {
            const directory = await pickProjectDirectory();
            if (directory) {
              setDirectories(current => current.includes(directory) ? current : [...current, directory]);
              setName(current => current || directory.split(/[\\/]/).filter(Boolean).pop() || "新项目");
            }
          } catch (cause) { setMessage(userFacingError(cause, "无法打开文件夹选择窗口，请重试。")); }
          finally { setBusy(false); setChoosingFolder(false); }
        }}>＋ 添加文件夹</button>}
        {!("__TAURI_INTERNALS__" in window) && <div className="project-directory-row"><input disabled={busy} aria-label="目录绝对路径" value={path} onChange={event => setPath(event.target.value)} placeholder="输入目录绝对路径" /><button type="button" disabled={busy || directories.length >= 32 || !path.trim()} onClick={() => { setDirectories(current => current.includes(path.trim()) ? current : [...current, path.trim()]); setPath(""); }}>添加</button></div>}
        </div>
        <p className="project-scope-note">选择一个主文件夹，也可以添加相关文件夹。Agent 对文件的访问遵循你设置的权限。</p>
        {message && <div className="error" role="alert">{message}</div>}
        <footer><button className="secondary" type="button" disabled={busy} onClick={onClose}>{t("common.cancel")}</button><button className="send" disabled={busy || !name.trim() || !directories.length}>{busy && !choosingFolder ? (project ? t("common.saving") : t("nav.creating")) : (project ? t("common.save") : t("nav.createProject"))}</button></footer>
      </form>
    </div>
  );
}
