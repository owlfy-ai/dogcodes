import type { ReactNode } from "react";
import { useI18n } from "./i18n";
import { providerLabel } from "./agentPresentation";

export interface PendingAgent { providerId: string; turnId?: string; connecting?: boolean }

export function AgentPending({ providerId, connecting = false, renderAgent }: {
  providerId: string;
  connecting?: boolean;
  renderAgent?: (providerId: string) => ReactNode;
}) {
  const { t, resolvedLanguage } = useI18n();
  const label = connecting ? t("agentPending.connecting", { agent: providerLabel(providerId, resolvedLanguage) }) : t("agentPending.responding");
  return <div className="response-agent-anchor agent-pending" role="status" aria-label={connecting ? label : t("agentPending.label")}>
    {renderAgent && <span className="response-agent-badge">{renderAgent(providerId)}</span>}
    <span className="agent-pending-label">{label}<span className="agent-pending-dots" aria-hidden="true"><i/><i/><i/></span></span>
  </div>;
}
