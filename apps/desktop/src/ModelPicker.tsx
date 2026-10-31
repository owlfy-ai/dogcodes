import { LoadingStatus } from "./LoadingStatus";
import { FloatingPopover } from "./FloatingPopover";
import { DropdownChevron } from "./DropdownChevron";
import { focusComposerInput } from "./composerFocus";
import { useEffect, useRef, useState } from "react";
import type { ModelCatalog } from "@dogcodes/protocol";
import { useI18n } from "./i18n";

// Known effort ids get translated labels; unknown ones keep the agent's label.
const knownEfforts = new Set(["off", "auto", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]);
export function ModelPicker({ catalog, model, thinking, loading, error, disabled, onChange, onRetry, onOpen, refreshDisabled = false }: {
  catalog: ModelCatalog; model: string | null; thinking: string | null;
  loading: boolean; error: string | null; disabled: boolean;
  onOpen?: () => void; refreshDisabled?: boolean;
  onChange: (model: string | null, thinking: string | null) => void; onRetry: () => void;
}) {
  const { t } = useI18n();
  const effortLabel = (id: string | null) => id && knownEfforts.has(id) ? t(`modelPicker.effort.${id}`) : undefined;
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
  const modelLabel = selected?.label ?? t(model ? "modelPicker.savedModel" : loading ? "modelPicker.selectModel" : defaultUnavailable ? "modelPicker.chooseModel" : "modelPicker.defaultModel");
  const thinkingLabel = effortLabel(effectiveThinking) ?? efforts.find(option => option.id === effectiveThinking)?.label ?? t(thinking ? "modelPicker.savedEffort" : "modelPicker.default");
  return <div className="model-picker" ref={root}>
    <button type="button" ref={trigger} className="model-trigger" data-tooltip={t("modelPicker.modelTooltip", { label: modelLabel })} aria-label={t("modelPicker.selectModel")} aria-haspopup="dialog" aria-expanded={open === "model"}
      disabled={disabled} onClick={() => openMenu("model")}>
      <svg className="composer-compact-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4m6-4v4M9 18v4m6-4v4M2 9h4m-4 6h4m12-6h4m-4 6h4M9 9h6v6H9z"/></svg><span className="composer-control-label">{modelLabel}</span><DropdownChevron />
    </button>
    {(efforts.length > 0 || thinking || open === "thinking") && <button type="button" ref={thinkingTrigger} className="thinking-trigger" data-tooltip={t("modelPicker.effortTooltip", { label: thinkingLabel })} aria-label={t("modelPicker.selectEffort")} aria-haspopup="dialog" aria-expanded={open === "thinking"}
      disabled={disabled || loading} onClick={() => openMenu("thinking")}>
      <svg className="composer-compact-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><path d="M9 18h6m-6 3h6M8 15c-6-5-2-13 4-13s10 8 4 13l-1 3H9zM12 6v5m-2-2 2 2 2-2"/></svg><span className="composer-control-label">{thinkingLabel}</span><DropdownChevron />
    </button>}
    {open && <FloatingPopover className="model-menu" role="dialog" aria-label={t(open === "model" ? "modelPicker.model" : "modelPicker.effort.title")}>
      <header><span>{t(open === "model" ? "modelPicker.model" : "modelPicker.effort.title")}</span>
        <button type="button" className="model-refresh" aria-label={t(open === "model" ? "modelPicker.refreshModels" : "modelPicker.refreshEfforts")}
          data-tooltip={t(refreshDisabled ? "modelPicker.refreshDisabled" : "modelPicker.refreshTooltip")}
          disabled={loading || refreshDisabled} onClick={onRetry}>
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M13 6a5 5 0 0 0-8.5-2L2 6m0-4v4h4m-3 4a5 5 0 0 0 8.5 2l2.5-2m0 4v-4h-4" /></svg>
        </button>
      </header>
      {loading && <LoadingStatus className="model-message">{t("modelPicker.loading")}</LoadingStatus>}
      {error && <p className="model-message" role="status">{t(catalog.models.length ? "modelPicker.refreshFailed" : "modelPicker.loadFailed")}</p>}
      {!loading && !error && (missingModel || missingThinking) && <p className="model-message" role="status">
        {t(missingModel ? !model && defaultUnavailable ? "modelPicker.defaultGone" : "modelPicker.modelMissing" : "modelPicker.effortMissing")}
      </p>}
      {open === "model" ? <>
        <button type="button" disabled={loading || defaultUnavailable} className={!model ? "selected" : ""} onClick={() => choose(null, null)}>{t(defaultUnavailable ? "modelPicker.defaultUnavailable" : "modelPicker.defaultRecommended")}<span>{!model && !defaultUnavailable ? "✓" : ""}</span></button>
        <div className="model-options">
          {catalog.models.map(option => <button type="button" disabled={loading} key={option.id} className={model === option.id ? "selected" : ""}
            onClick={() => choose(option.id, option.defaultThinking ?? null)}>{option.label}<span>{model === option.id ? "✓" : ""}</span></button>)}
        </div>
        {!loading && !error && !catalog.models.length && !defaultUnavailable && <p className="model-message">{t("modelPicker.agentDefaultOnly")}</p>}
      </> : <>
        {missingThinking && <button type="button" disabled={loading} onClick={() => choose(model ?? selected?.id ?? null, null)}>{t("modelPicker.useDefaultEffort")}</button>}
        <div className="model-options">
          {efforts.map(option => <button type="button" disabled={loading} key={option.id} className={effectiveThinking === option.id ? "selected" : ""}
            onClick={() => choose(model ?? selected?.id ?? null, option.id)}>{effortLabel(option.id) ?? option.label}<span>{effectiveThinking === option.id ? "✓" : ""}</span></button>)}
        </div>
        {!loading && !error && !efforts.length && <p className="model-message">{t("modelPicker.noEfforts")}</p>}
      </>}
    </FloatingPopover>}
  </div>;
}
