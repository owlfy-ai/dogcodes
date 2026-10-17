import type { ReactNode } from "react";

export interface PendingAgent { providerId: string; turnId?: string }

export function AgentPending({ providerId, renderAgent }: {
  providerId: string;
  renderAgent?: (providerId: string) => ReactNode;
}) {
  return <div className="response-agent-anchor agent-pending" role="status" aria-label="正在等待 Agent 响应">
    {renderAgent && <span className="response-agent-badge">{renderAgent(providerId)}</span>}
    <span className="agent-pending-label">正在响应<span className="agent-pending-dots" aria-hidden="true"><i/><i/><i/></span></span>
  </div>;
}
