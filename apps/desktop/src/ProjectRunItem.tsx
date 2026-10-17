import { useEffect, useRef, useState } from "react";
import type { ProjectRunCommand, ProjectRunEdit } from "./projectRunClient";

function Icon({ path }: { path: string }) {
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d={path} /></svg>;
}
export function ProjectRunItem({ command, path, running, disabled, onRun, onEdit }: {
  command: ProjectRunCommand; path: string; running: boolean; disabled: boolean;
  onRun: () => void; onEdit: (edit: ProjectRunEdit) => Promise<boolean>;
}) {
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
      <input ref={input} aria-label="运行项名称" value={name} maxLength={60} disabled={disabled} onChange={event => setName(event.target.value)}
        onKeyDown={event => { if (event.nativeEvent.isComposing) { if (event.key === "Enter") event.preventDefault(); return; } if (event.key === "Escape") { event.preventDefault(); setRenaming(false); } }} />
      <button type="submit" aria-label="保存名称" data-tooltip="保存名称" disabled={disabled || !name.trim()}><Icon path="m5 12 4 4L19 6" /></button>
      <button type="button" aria-label="取消重命名" data-tooltip="取消重命名" disabled={disabled} onClick={() => setRenaming(false)}><Icon path="m6 6 12 12M18 6 6 18" /></button>
    </form> : <>
      <button type="button" className="project-run-command" disabled={disabled}
        data-tooltip={`${command.command}\n目录：${path}${command.cwd === "." ? "" : `/${command.cwd}`}\n来源：${command.source}`} onClick={onRun}>
        <Icon path="m8 5 11 7-11 7Z" /><span>{command.label}</span>{running && <small>运行中</small>}
      </button>
      <div className="project-run-item-actions">
        <button type="button" aria-label={`重命名 ${command.label}`} data-tooltip="重命名" disabled={disabled} onClick={() => { setName(command.label); setRenaming(true); }}><Icon path="m15 5 4 4M4 20l4-1L20 7a2.8 2.8 0 0 0-4-4L4 15v5Z" /></button>
        <button type="button" className={command.pinned ? "is-pinned" : undefined} aria-pressed={Boolean(command.pinned)} aria-label={`${command.pinned ? "取消固定" : "固定"} ${command.label}`} data-tooltip={command.pinned ? "取消固定" : "固定，重新分析时保留"} disabled={disabled} onClick={() => void onEdit({ action: "pin", pinned: !command.pinned })}><Icon path="M16 9V4l1-1H7l1 1v5l-3 3v2h14v-2l-3-3ZM12 14v8" /></button>
        <button type="button" aria-label={`删除 ${command.label}`} data-tooltip="删除运行项" disabled={disabled} onClick={() => void onEdit({ action: "delete" })}><Icon path="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 10v7M14 10v7" /></button>
      </div>
    </>}
  </div>;
}

export function RunAnalysisConfirmation({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { const element = dialog.current; element?.showModal(); return () => element?.close(); }, []);
  return <dialog ref={dialog} className="project-run-confirm" aria-labelledby="run-analysis-confirm-title" onCancel={event => { event.preventDefault(); onCancel(); }}>
    <h3 id="run-analysis-confirm-title">重新分析运行项？</h3>
    <p>你已删除或重命名过运行项。重新分析会覆盖未固定的运行项，已删除的运行项可能重新出现，未固定项的自定义名称也会被替换。</p>
    <p>已固定的运行项及其名称、命令将保留。</p>
    <footer><button type="button" autoFocus onClick={onCancel}>取消</button><button type="button" onClick={onConfirm}>重新分析</button></footer>
  </dialog>;
}
