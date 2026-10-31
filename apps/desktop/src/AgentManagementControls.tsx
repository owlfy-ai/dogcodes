import { t, useI18n, type ResolvedLanguage } from "./i18n";
import type { DragEvent, KeyboardEvent } from "react";
import { providerLabel } from "./agentPresentation";
import { moveAgent, setAgentEnabled, useAgentPreferencesWriteState } from "./agentPreferences";

export const AGENT_DRAG_TYPE = "application/x-dogcodes-agent";
export function AgentManagementControls({ providerId, enabled, order, onDragStart, onDragEnd }: {
  providerId: string; enabled: boolean; order: string[];
  onDragStart: (event: DragEvent<HTMLButtonElement>) => void; onDragEnd: () => void;
}) {
  const { resolvedLanguage: lang } = useI18n();
  const label = providerLabel(providerId, lang);
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
      aria-label={t("agentUi.reorder", { name: label }, lang)} data-tooltip={t("agentUi.dragReorder", undefined, lang)}>
      <svg viewBox="0 0 16 20" width="14" height="18" fill="currentColor" aria-hidden="true"><circle cx="5" cy="4" r="1.3"/><circle cx="11" cy="4" r="1.3"/><circle cx="5" cy="10" r="1.3"/><circle cx="11" cy="10" r="1.3"/><circle cx="5" cy="16" r="1.3"/><circle cx="11" cy="16" r="1.3"/></svg>
    </button>
    <span className="agent-enabled-label">{enabled ? t("agentUi.enabled", undefined, lang) : t("agentUi.disabled", undefined, lang)}</span>
    <button type="button" role="switch" className="agent-enabled-switch" aria-label={t("agentUi.enableAgent", { name: label }, lang)} aria-checked={enabled}
      data-tooltip={enabled ? t("agentUi.disableHint", undefined, lang) : t("agentUi.enableHint", undefined, lang)}
      onClick={() => setAgentEnabled(providerId, !enabled)}><span /></button>
  </div>;
}
