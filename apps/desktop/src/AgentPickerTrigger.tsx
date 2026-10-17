import { AgentIcon } from "./agentPresentation";
import "./reviewActions.css";

export function AgentPickerTrigger({ providerId, label, tooltip, open, disabled, onClick }: {
  providerId?: string; label: string; tooltip: string; open: boolean; disabled?: boolean; onClick: () => void;
}) {
  return <button className="turn-action agent-picker-trigger" type="button" aria-label={label} data-tooltip={tooltip} aria-expanded={open} disabled={disabled} onClick={onClick}>
    <AgentIcon providerId={providerId ?? "dogcodes"}/>
    <svg className="agent-picker-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m4 6 4 4 4-4"/></svg>
  </button>;
}
