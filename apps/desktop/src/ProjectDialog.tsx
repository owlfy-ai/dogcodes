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
        if (!client.updateProject) throw new Error(t("projectDialog.editUnsupported"));
        await client.updateProject(project, name.trim(), directories[0], directories.slice(1));
        await onReady(project.projectId);
      } else {
        const projectId = await client.registerProject(name.trim(), directories[0], directories.slice(1));
        await onReady(projectId);
      }
    } catch (cause) {
      setMessage(userFacingError(cause, t(project ? "projectDialog.saveFailed" : "projectDialog.createFailed")));
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
        <label>{t("projectDialog.name")}<input required maxLength={120} disabled={busy} placeholder={t("projectDialog.namePlaceholder")} value={name} onChange={(event) => setName(event.target.value)} /></label>
        <label>{t("projectDialog.directories")}</label>
        <div className="project-directory-list">
          {directories.map((directory, index) => <div className="project-directory-row" key={directory}>
            <FolderIcon /><span data-tooltip={directory}>{directory}<small>{t(index === 0 ? "projectDialog.primary" : "projectDialog.additional")}</small></span>
            {index > 0 && <button type="button" disabled={busy} aria-label={t("projectDialog.makePrimaryLabel", { directory })} onClick={() => setDirectories([directory, ...directories.filter(d => d !== directory)])}>{t("projectDialog.makePrimary")}</button>}
            <button type="button" disabled={busy} aria-label={t("projectDialog.remove", { directory })} onClick={() => setDirectories(directories.filter(d => d !== directory))}>×</button>
          </div>)}
          {directories.length === 0 && <p>{t("projectDialog.emptyHint")}</p>}
        {"__TAURI_INTERNALS__" in window && <button type="button" disabled={busy || directories.length >= 32} onClick={async () => {
          setBusy(true);
          setChoosingFolder(true);
          setMessage(null);
          try {
            const directory = await pickProjectDirectory();
            if (directory) {
              setDirectories(current => current.includes(directory) ? current : [...current, directory]);
              setName(current => current || directory.split(/[\\/]/).filter(Boolean).pop() || t("projectDialog.defaultName"));
            }
          } catch (cause) { setMessage(userFacingError(cause, t("projectDialog.pickerFailed"))); }
          finally { setBusy(false); setChoosingFolder(false); }
        }}>＋ {t("projectDialog.addFolder")}</button>}
        {!("__TAURI_INTERNALS__" in window) && <div className="project-directory-row"><input disabled={busy} aria-label={t("projectDialog.absolutePath")} value={path} onChange={event => setPath(event.target.value)} placeholder={t("projectDialog.absolutePathPlaceholder")} /><button type="button" disabled={busy || directories.length >= 32 || !path.trim()} onClick={() => { setDirectories(current => current.includes(path.trim()) ? current : [...current, path.trim()]); setPath(""); }}>{t("projectDialog.add")}</button></div>}
        </div>
        <p className="project-scope-note">{t("projectDialog.scopeNote")}</p>
        {message && <div className="error" role="alert">{message}</div>}
        <footer><button className="secondary" type="button" disabled={busy} onClick={onClose}>{t("common.cancel")}</button><button className="send" disabled={busy || !name.trim() || !directories.length}>{busy && !choosingFolder ? (project ? t("common.saving") : t("nav.creating")) : (project ? t("common.save") : t("nav.createProject"))}</button></footer>
      </form>
    </div>
  );
}
