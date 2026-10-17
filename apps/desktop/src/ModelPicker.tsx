import { LoadingStatus } from "./LoadingStatus";
import { FloatingPopover } from "./FloatingPopover";
import { DropdownChevron } from "./DropdownChevron";
import { focusComposerInput } from "./composerFocus";
import { useEffect, useRef, useState } from "react";
import type { ModelCatalog } from "@dogcodes/protocol";

const effortLabels: Record<string, string> = { off: "关闭", auto: "自动", low: "低", medium: "中", high: "高", xhigh: "超高", max: "最高" };
export function ModelPicker({ catalog, model, thinking, loading, error, disabled, onChange, onRetry, onOpen, refreshDisabled = false }: {
  catalog: ModelCatalog; model: string | null; thinking: string | null;
  loading: boolean; error: string | null; disabled: boolean;
  onOpen?: () => void; refreshDisabled?: boolean;
  onChange: (model: string | null, thinking: string | null) => void; onRetry: () => void;
}) {
  const [open, setOpen] = useState<"model" | "thinking" | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const thinkingTrigger = useRef<HTMLButtonElement>(null);
  const restoreFocus = () => (open === "thinking" ? thinkingTrigger : trigger).current?.focus();
  const selected = catalog.models.find(option => option.id === (model ?? catalog.defaultModel));
  const efforts = selected?.thinkingOptions ?? [];
  const effectiveThinking = thinking ?? selected?.defaultThinking ?? null;
  const defaultUnavailable = !!catalog.defaultModel && !catalog.models.some(option => option.id === catalog.defaultModel);
  const missingModel = !!(model ?? catalog.defaultModel) && !selected;
  const missingThinking = !!thinking && !efforts.some(option => option.id === thinking);
  const openMenu = (menu: "model" | "thinking") => {
    if (open !== menu) {
      if (error && !loading && !refreshDisabled) onRetry(); else onOpen?.();
    }
    setOpen(open === menu ? null : menu);
  };
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(null); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.stopPropagation(); setOpen(null); restoreFocus(); } };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open]);
  useEffect(() => { if (disabled) setOpen(null); }, [disabled]);
  const choose = (id: string | null, effort: string | null) => {
    onChange(id, effort); setOpen(null);
    if (!focusComposerInput(root.current)) restoreFocus();
  };
  const modelLabel = selected?.label ?? (model ? "已保存的模型" : loading ? "选择模型" : defaultUnavailable ? "请选择模型" : "默认模型");
  const thinkingLabel = effortLabels[effectiveThinking ?? ""] ?? efforts.find(option => option.id === effectiveThinking)?.label ?? (thinking ? "已保存的强度" : "默认");
  return <div className="model-picker" ref={root}>
    <button type="button" ref={trigger} className="model-trigger" data-tooltip={`模型：${modelLabel}`} aria-label="选择模型" aria-haspopup="dialog" aria-expanded={open === "model"}
      disabled={disabled} onClick={() => openMenu("model")}>
      <svg className="composer-compact-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4m6-4v4M9 18v4m6-4v4M2 9h4m-4 6h4m12-6h4m-4 6h4M9 9h6v6H9z"/></svg><span className="composer-control-label">{modelLabel}</span><DropdownChevron />
    </button>
    {(efforts.length > 0 || thinking || open === "thinking") && <button type="button" ref={thinkingTrigger} className="thinking-trigger" data-tooltip={`思考强度：${thinkingLabel}`} aria-label="选择思考强度" aria-haspopup="dialog" aria-expanded={open === "thinking"}
      disabled={disabled || loading} onClick={() => openMenu("thinking")}>
      <svg className="composer-compact-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><path d="M9 18h6m-6 3h6M8 15c-6-5-2-13 4-13s10 8 4 13l-1 3H9zM12 6v5m-2-2 2 2 2-2"/></svg><span className="composer-control-label">{thinkingLabel}</span><DropdownChevron />
    </button>}
    {open && <FloatingPopover className="model-menu" role="dialog" aria-label={open === "model" ? "模型" : "思考强度"}>
      <header><span>{open === "model" ? "模型" : "思考强度"}</span>
        <button type="button" className="model-refresh" aria-label={open === "model" ? "刷新模型列表" : "刷新思考强度列表"}
          data-tooltip={refreshDisabled ? "Agent 运行中，结束后可刷新" : "重新获取模型和当前模型的思考强度"}
          disabled={loading || refreshDisabled} onClick={onRetry}>
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M13 6a5 5 0 0 0-8.5-2L2 6m0-4v4h4m-3 4a5 5 0 0 0 8.5 2l2.5-2m0 4v-4h-4" /></svg>
        </button>
      </header>
      {loading && <LoadingStatus className="model-message">正在获取可用模型与思考强度…</LoadingStatus>}
      {error && <p className="model-message" role="status">{catalog.models.length ? "刷新失败，保留上次列表，请重试。" : "暂时无法获取模型，请重试。"}</p>}
      {!loading && !error && (missingModel || missingThinking) && <p className="model-message" role="status">
        {missingModel ? !model && defaultUnavailable ? "默认模型已不可用，请选择其他模型。" : "已选模型不在当前列表中，原选择已保留，请重新选择。" : "已选思考强度不在当前列表中，原选择已保留，请重新选择。"}
      </p>}
      {open === "model" ? <>
        <button type="button" disabled={loading || defaultUnavailable} className={!model ? "selected" : ""} onClick={() => choose(null, null)}>{defaultUnavailable ? "默认模型不可用" : "默认（推荐）"}<span>{!model && !defaultUnavailable ? "✓" : ""}</span></button>
        <div className="model-options">
          {catalog.models.map(option => <button type="button" disabled={loading} key={option.id} className={model === option.id ? "selected" : ""}
            onClick={() => choose(option.id, option.defaultThinking ?? null)}>{option.label}<span>{model === option.id ? "✓" : ""}</span></button>)}
        </div>
        {!loading && !error && !catalog.models.length && !defaultUnavailable && <p className="model-message">当前 Agent 使用默认模型，无需额外设置</p>}
      </> : <>
        {missingThinking && <button type="button" disabled={loading} onClick={() => choose(model ?? selected?.id ?? null, null)}>使用默认思考强度</button>}
        <div className="model-options">
          {efforts.map(option => <button type="button" disabled={loading} key={option.id} className={effectiveThinking === option.id ? "selected" : ""}
            onClick={() => choose(model ?? selected?.id ?? null, option.id)}>{effortLabels[option.id] ?? option.label}<span>{effectiveThinking === option.id ? "✓" : ""}</span></button>)}
        </div>
        {!loading && !error && !efforts.length && <p className="model-message">当前模型未提供可选思考强度</p>}
      </>}
    </FloatingPopover>}
  </div>;
}
