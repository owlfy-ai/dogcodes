import { enabledAgents, useAgentPreferences } from "./agentPreferences";
import { LoadingStatus } from "./LoadingStatus";
import { useContext } from "react";
import type { ExecutorRef, QuotaSnapshotView } from "@dogcodes/protocol";
import { AgentIcon, providerLabel } from "./agentPresentation";
import { AgentQuotaContext, AgentQuotaRing } from "./AgentQuotaRing";
import { FloatingPopover } from "./FloatingPopover";
import "./agentPicker.css";

export const agentChoiceKey = (executor: ExecutorRef | null | undefined) => executor ? `${executor.providerId}:${executor.installationId ?? ""}` : "";
export interface AgentPickerOption {
  key: string;
  executor: ExecutorRef;
  quota?: QuotaSnapshotView["quota"];
}

/** The same menu for conversation, review, commit messages and run analysis.
 * Reading the context never starts a quota probe. Callers own selection effects.
 */
export function AgentPickerMenu({ options, selectedKey, onSelect, title, label, quotas, loading = false, placement, role = "group" }: {
  options: AgentPickerOption[];
  selectedKey: string;
  onSelect: (option: AgentPickerOption) => void;
  title: string;
  label: string;
  quotas?: QuotaSnapshotView[];
  loading?: boolean;
  placement?: "bottom";
  role?: "group" | "dialog";
}) {
  const cachedQuotas = useContext(AgentQuotaContext);
  const readings = quotas ?? cachedQuotas;
  const visibleOptions = enabledAgents(options, option => option.executor.providerId, useAgentPreferences());
  return <FloatingPopover className="agent-picker-menu" data-agent-picker role={role} aria-label={label} placement={placement}>
    <header>{title}</header>
    {loading ? <LoadingStatus>正在读取 Agent…</LoadingStatus> : visibleOptions.map(option => {
      const chosen = option.key === selectedKey;
      const quota = option.quota !== undefined ? option.quota : readings.find(view => view.providerId === option.executor.providerId && view.installationId === option.executor.installationId)?.quota ?? null;
      return <button type="button" key={option.key} aria-label={providerLabel(option.executor.providerId)} className={`agent-picker-option${chosen ? " active" : ""}`} aria-pressed={chosen} onClick={() => onSelect(option)}>
        <AgentIcon providerId={option.executor.providerId} size="small"/>
        <span className="agent-option-label"><b>{providerLabel(option.executor.providerId)}</b></span>
        <AgentQuotaRing quota={quota} providerId={option.executor.providerId}/>{chosen && <i>✓</i>}
      </button>;
    })}
    {!loading && !visibleOptions.length && <p>暂无可用 Agent，请在 Agent 管理中检测或启用</p>}
  </FloatingPopover>;
}
