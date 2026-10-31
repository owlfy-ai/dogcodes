import type { ReactNode } from "react";
import { translate as t } from "./i18n";

export interface PendingAgent { providerId: string; turnId?: string }

export function AgentPending({ providerId, renderAgent }: {
  providerId: string;
  renderAgent?: (providerId: string) => ReactNode;
}) {
  return <div className="response-agent-anchor agent-pending" role="status" aria-label={t("agentPending.label")}>
    {renderAgent && <span className="response-agent-badge">{renderAgent(providerId)}</span>}
    <span className="agent-pending-label">{t("agentPending.responding")}<span className="agent-pending-dots" aria-hidden="true"><i/><i/><i/></span></span>
  </div>;
}
