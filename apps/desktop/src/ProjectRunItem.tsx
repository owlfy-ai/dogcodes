import { useEffect, useRef, useState } from "react";
import type { ProjectRunCommand, ProjectRunEdit } from "./projectRunClient";
import { useI18n } from "./i18n";
import { ConfirmDialog } from "./AppDialog";
import { copyText } from "./clipboard";
import { showContextMenu } from "./ContextMenu";

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
  const startRenaming = () => { setName(command.label); setRenaming(true); };
  return <div className="project-run-row" data-pinned={command.pinned || undefined} onContextMenu={renaming ? undefined : event => showContextMenu(event, [
    { label: t(running ? "contextMenu.showOutput" : "contextMenu.run"), disabled, onSelect: onRun },
    { label: t("contextMenu.copyCommand"), onSelect: () => copyText(command.command) },
    "separator",
    { label: t("projectRun.rename"), disabled, onSelect: startRenaming },
    { label: t(command.pinned ? "projectRun.unpin" : "contextMenu.pinCommand"), disabled, onSelect: () => onEdit({ action: "pin", pinned: !command.pinned }) },
    { label: t("projectRun.delete"), disabled, onSelect: () => onEdit({ action: "delete" }) },
  ])}>
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
        <button type="button" aria-label={t("projectRun.renameItem", { label: command.label })} data-tooltip={t("projectRun.rename")} disabled={disabled} onClick={startRenaming}><Icon path="m15 5 4 4M4 20l4-1L20 7a2.8 2.8 0 0 0-4-4L4 15v5Z" /></button>
        <button type="button" className={command.pinned ? "is-pinned" : undefined} aria-pressed={Boolean(command.pinned)} aria-label={t(command.pinned ? "projectRun.unpinItem" : "projectRun.pinItem", { label: command.label })} data-tooltip={t(command.pinned ? "projectRun.unpin" : "projectRun.pinTooltip")} disabled={disabled} onClick={() => void onEdit({ action: "pin", pinned: !command.pinned })}><Icon path="M16 9V4l1-1H7l1 1v5l-3 3v2h14v-2l-3-3ZM12 14v8" /></button>
        <button type="button" aria-label={t("projectRun.deleteItem", { label: command.label })} data-tooltip={t("projectRun.delete")} disabled={disabled} onClick={() => void onEdit({ action: "delete" })}><Icon path="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 10v7M14 10v7" /></button>
      </div>
    </>}
  </div>;
}

export function RunAnalysisConfirmation({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  const { t } = useI18n();
  return <ConfirmDialog title={t("projectRun.reanalyzeTitle")} icon="refresh" confirmLabel={t("projectRun.reanalyze")} onCancel={onCancel} onConfirm={onConfirm}>
    <p>{t("projectRun.reanalyzeWarning")}</p>
    <p>{t("projectRun.reanalyzeKeeps")}</p>
  </ConfirmDialog>;
}

export function RunCloseConfirmation({ label, onCancel, onConfirm }: { label: string; onCancel: () => void; onConfirm: () => void }) {
  const { t } = useI18n();
  return <ConfirmDialog title={t("projectRun.closeRunningTitle")} icon="stop" focusConfirm confirmLabel={t("projectRun.stopAndClose")} onCancel={onCancel} onConfirm={onConfirm}>
    <p>{t("projectRun.closeRunningWarning", { label })}</p>
  </ConfirmDialog>;
}
