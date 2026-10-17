import { useEffect, useRef } from "react";
import type { ExecutorRef, QuotaSnapshotView, ReviewKind } from "@dogcodes/protocol";
import { providerLabel } from "./agentPresentation";
import { AgentPickerTrigger } from "./AgentPickerTrigger";
import { AgentPickerMenu } from "./AgentPickerMenu";
import "./reviewActions.css";

export const reviewLabels = { code: "审查代码", security: "安全检查", performance: "性能分析" };
export const reviewKinds = ["code", "security", "performance"] as const;
const executorKey = (executor: ExecutorRef) => `${executor.providerId}:${executor.installationId ?? ""}`;

export function ActionIcon({ name }: { name: string }) {
  const paths: Record<string, string> = {
    fork: "M3 10h6l7-7 M11 3h5v5 M9 10l7 7 M11 17h5v-5",
    copy: "M7 6h9v12H7z M4 14V3h9",
    diff: "M4 3h12v15H4z M7 8h6 M7 13h6 M10 10v6", code: "m6 5-4 5 4 5 M14 5l4 5-4 5 M12 3 8 17",
    security: "M10 2 3 5v5c0 4 7 8 7 8s7-4 7-8V5z M7 10l2 2 4-4",
    performance: "M3 16a8 8 0 1 1 14 0 M10 12l4-5 M5 13h1 M14 13h1 M10 5v1",
    more: "M3 10h1 M9 10h1 M15 10h1",
  };
  return <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name] ?? paths.more}/></svg>;
}

export function ReviewActionButton({ kind, disabled, tooltip, onClick }: {
  kind: ReviewKind; disabled: boolean; tooltip: string; onClick: () => void;
}) {
  return <button type="button" className="turn-action" aria-label={reviewLabels[kind]} data-tooltip={tooltip} disabled={disabled} onClick={onClick}><ActionIcon name={kind}/></button>;
}

export function ReviewAgentPicker({ executors, executor, quotas, open, onOpenChange, onSelect, label = "选择审查 Agent" }: {
  executors: ExecutorRef[]; executor: ExecutorRef | undefined; quotas: QuotaSnapshotView[];
  open: boolean; onOpenChange: (open: boolean) => void; onSelect: (key: string) => void; label?: string;
}) {
  const picker = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!picker.current?.contains(event.target as Node)) onOpenChange(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { onOpenChange(false); picker.current?.querySelector("button")?.focus(); } };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open, onOpenChange]);
  return <div className="turn-review-agent" ref={picker}>
    <AgentPickerTrigger providerId={executor?.providerId} label={label} tooltip={executor ? `审查 Agent：${providerLabel(executor.providerId)}，独立于主 Agent` : "暂无可用 Agent，请先连接或重新检测 Agent"} open={open} onClick={() => onOpenChange(!open)}/>
    {open && <AgentPickerMenu options={executors.map(candidate => ({ key: executorKey(candidate), executor: candidate }))}
      selectedKey={executor ? executorKey(executor) : ""} quotas={quotas} title="选择审查使用的 Agent" label="审查 Agent 列表" onSelect={option => {
        onSelect(option.key);
        try { localStorage.setItem("dogcodes.review.executor", option.key); } catch { /* Optional preference. */ }
        onOpenChange(false);
        picker.current?.querySelector("button")?.focus();
      }}/>}
  </div>;
}
