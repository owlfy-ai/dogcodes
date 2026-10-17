import { useEffect, useState, type ReactNode } from "react";
import { DeferredDetails } from "./DeferredDetails";

/** Shared work summary for the main conversation and review tool activity. */
export function TurnProcess({ startedAt, completedAt, active = false, agent, children }: {
  startedAt: string; completedAt?: string | null; active?: boolean; agent?: ReactNode; children: () => ReactNode;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [active]);
  const end = active ? now : completedAt ? Date.parse(completedAt) : Number.NaN;
  const seconds = Math.max(0, Math.round((end - Date.parse(startedAt)) / 1000));
  const label = `${active ? "正在工作" : "已工作"}${Number.isFinite(seconds) ? ` ${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒` : ""}`;
  return <DeferredDetails className="turn-process" summary={
    <summary className="turn-process-heading">{agent && <span className="turn-process-agent">{agent}</span>}{label}<svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="m6 4 4 4-4 4"/></svg></summary>}>
    {() => <div className="conversation-flow turn-process-body">{children()}</div>}
  </DeferredDetails>;
}
