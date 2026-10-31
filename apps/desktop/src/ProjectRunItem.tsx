import { useEffect, useRef, useState } from "react";
import type { ProjectRunCommand, ProjectRunEdit } from "./projectRunClient";
import { useI18n } from "./i18n";

function Icon({ path }: { path: string }) {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d={path} /></svg>;
}
export function ProjectRunItem({ command, path, running, disabled, onRun, onEdit }: {
  command: ProjectRunCommand; path: string; running: boolean; disabled: boolean;
  onRun: () => void; onEdit: (edit: ProjectRunEdit) => Promise<boolean>;
}) {
  const { t } = useI18n();
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(command.label);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { if (renaming) { input.current?.focus(); input.current?.select(); } }, [renaming]);
  const save = async () => {
    if (!name.trim() || disabled) return;
    if (name.trim() === command.label || await onEdit({ action: "rename", label: name.trim() })) setRenaming(false);
  };
  return <div className="project-run-row" data-pinned={command.pinned || undefined}>
    {renaming ? <form className="project-run-rename" onSubmit={event => { event.preventDefault(); void save(); }}>
      <input ref={input} aria-label={t("projectRun.nameLabel")} value={name} maxLength={60} disabled={disabled} onChange={event => setName(event.target.value)}
        onKeyDown={event => { if (event.nativeEvent.isComposing) { if (event.key === "Enter") event.preventDefault(); return; } if (event.key === "Escape") { event.preventDefault(); setRenaming(false); } }} />
      <button type="submit" aria-label={t("projectRun.saveName")} data-tooltip={t("projectRun.saveName")} disabled={disabled || !name.trim()}><Icon path="m5 12 4 4L19 6" /></button>
      <button type="button" aria-label={t("projectRun.cancelRename")} data-tooltip={t("projectRun.cancelRename")} disabled={disabled} onClick={() => setRenaming(false)}><Icon path="m6 6 12 12M18 6 6 18" /></button>
    </form> : <>
      <button type="button" className="project-run-command" disabled={disabled}
        data-tooltip={t("projectRun.commandTooltip", { command: command.command, path: `${path}${command.cwd === "." ? "" : `/${command.cwd}`}`, source: command.source })} onClick={onRun}>
        <Icon path="m8 5 11 7-11 7Z" /><span>{command.label}</span>{running && <small>{t("projectRun.running")}</small>}
      </button>
      <div className="project-run-item-actions">
        <button type="button" aria-label={t("projectRun.renameItem", { label: command.label })} data-tooltip={t("projectRun.rename")} disabled={disabled} onClick={() => { setName(command.label); setRenaming(true); }}><Icon path="m15 5 4 4M4 20l4-1L20 7a2.8 2.8 0 0 0-4-4L4 15v5Z" /></button>
        <button type="button" className={command.pinned ? "is-pinned" : undefined} aria-pressed={Boolean(command.pinned)} aria-label={t(command.pinned ? "projectRun.unpinItem" : "projectRun.pinItem", { label: command.label })} data-tooltip={t(command.pinned ? "projectRun.unpin" : "projectRun.pinTooltip")} disabled={disabled} onClick={() => void onEdit({ action: "pin", pinned: !command.pinned })}><Icon path="M16 9V4l1-1H7l1 1v5l-3 3v2h14v-2l-3-3ZM12 14v8" /></button>
        <button type="button" aria-label={t("projectRun.deleteItem", { label: command.label })} data-tooltip={t("projectRun.delete")} disabled={disabled} onClick={() => void onEdit({ action: "delete" })}><Icon path="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 10v7M14 10v7" /></button>
      </div>
    </>}
  </div>;
}

export function RunAnalysisConfirmation({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  const { t } = useI18n();
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { const element = dialog.current; element?.showModal(); return () => element?.close(); }, []);
  return <dialog ref={dialog} className="project-run-confirm" aria-labelledby="run-analysis-confirm-title" onCancel={event => { event.preventDefault(); onCancel(); }}>
    <h3 id="run-analysis-confirm-title">{t("projectRun.reanalyzeTitle")}</h3>
    <p>{t("projectRun.reanalyzeWarning")}</p>
    <p>{t("projectRun.reanalyzeKeeps")}</p>
    <footer><button type="button" autoFocus onClick={onCancel}>{t("common.cancel")}</button><button type="button" onClick={onConfirm}>{t("projectRun.reanalyze")}</button></footer>
  </dialog>;
}
