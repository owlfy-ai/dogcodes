import type { DragEvent, KeyboardEvent } from "react";
import { providerLabel } from "./agentPresentation";
import { moveAgent, setAgentEnabled, useAgentPreferencesWriteState } from "./agentPreferences";

export const AGENT_DRAG_TYPE = "application/x-dogcodes-agent";
export function AgentManagementControls({ providerId, enabled, order, onDragStart, onDragEnd }: {
  providerId: string; enabled: boolean; order: string[];
  onDragStart: (event: DragEvent<HTMLButtonElement>) => void; onDragEnd: () => void;
}) {
  const label = providerLabel(providerId);
  const settings = useAgentPreferencesWriteState();
  const saving = settings.busy || settings.unconfirmed;
  const move = (event: KeyboardEvent) => {
    const step = event.key === "ArrowUp" || event.key === "ArrowLeft" ? -1 : event.key === "ArrowDown" || event.key === "ArrowRight" ? 1 : 0;
    if (!step || saving) return;
    event.preventDefault();
    const target = order[order.indexOf(providerId) + step];
    if (target) moveAgent(providerId, target, order);
  };
  return <div className="agent-management-controls">
    <button type="button" className="agent-drag-handle" draggable={!saving} disabled={saving} onDragStart={onDragStart} onDragEnd={onDragEnd} onKeyDown={move}
      aria-label={`调整 ${label} 排序`} data-tooltip="拖拽排序">
      <svg viewBox="0 0 16 20" width="14" height="18" fill="currentColor" aria-hidden="true"><circle cx="5" cy="4" r="1.3"/><circle cx="11" cy="4" r="1.3"/><circle cx="5" cy="10" r="1.3"/><circle cx="11" cy="10" r="1.3"/><circle cx="5" cy="16" r="1.3"/><circle cx="11" cy="16" r="1.3"/></svg>
    </button>
    <span className="agent-enabled-label">{enabled ? "已启用" : "已禁用"}</span>
    <button type="button" role="switch" className="agent-enabled-switch" aria-label={`启用 ${label}`} aria-checked={enabled}
      data-tooltip={enabled ? "禁用后将从 Agent 列表中隐藏" : "启用后可在 Agent 列表中选择"}
      onClick={() => setAgentEnabled(providerId, !enabled)}><span /></button>
  </div>;
}
