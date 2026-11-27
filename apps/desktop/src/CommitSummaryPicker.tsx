import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ModelCatalog, ModelOption } from "@dogcodes/protocol";
import { availableAgentExecutors } from "./availableAgentExecutors";
import { enabledAgents, readAgentPreferences, useAgentPreferences, type AgentPreferences } from "./agentPreferences";
import { AgentPickerMenu, agentChoiceKey } from "./AgentPickerMenu";
import { AgentIcon, providerLabel } from "./agentPresentation";
import { FloatingPopover } from "./FloatingPopover";
import { LoadingStatus } from "./LoadingStatus";
import { COMMIT_AGENT_KEY, readSummaryAgent, readSummaryModel, saveSummaryModel, type SummaryExecutor } from "./CommitSummarySettings";
import { translate as t } from "./i18n";
import "./commitSummaryPicker.css";

/** What a commit message is generated with. A null model lets the Agent use its own default. */
export interface SummaryChoice { executor: SummaryExecutor; model: string | null }

const usableModels = (catalog: ModelCatalog | null | undefined): ModelOption[] =>
  Array.isArray(catalog?.models) ? catalog.models.filter(option => !option.unavailableReason) : [];
// A saved model still offered wins; otherwise the first listed model.
function modelFor(executor: SummaryExecutor, catalog: ModelCatalog | null | undefined): string | null {
  const models = usableModels(catalog), stored = readSummaryModel(executor);
  if (stored && (!catalog || models.some(option => option.id === stored))) return stored;
  return models[0]?.id ?? null;
}
function agentFor(agents: SummaryExecutor[] | null, saved: SummaryExecutor | null, preferences: AgentPreferences, fallbackProviderId?: string) {
  const visible = agents ? enabledAgents(agents, item => item.providerId, preferences) : [];
  if (saved && !preferences.disabled.includes(saved.providerId) && (!agents || visible.some(item => agentChoiceKey(item) === agentChoiceKey(saved))))
    return agents ? visible.find(item => agentChoiceKey(item) === agentChoiceKey(saved))! : saved;
  if (fallbackProviderId === undefined || !agents) return null;
  return visible.find(item => item.providerId === fallbackProviderId) ?? visible[0] ?? null;
}

/** Agent and model selection for commit messages, shared by every place that writes one.
 * Without `fallbackProviderId` nothing is picked until the user chooses an Agent. */
export function useCommitSummaryChoice(taskId: string, options: { fallbackProviderId?: string; enabled?: boolean; onError?: (cause: unknown) => void } = {}) {
  const { fallbackProviderId, enabled = true } = options;
  const preferences = useAgentPreferences();
  const [agents, setAgents] = useState<SummaryExecutor[] | null>(null);
  const [loadingAgents, setLoadingAgents] = useState(false);
  const [saved, setSaved] = useState(readSummaryAgent);
  const [agentMenu, setAgentMenuState] = useState(false);
  const [modelMenu, setModelMenu] = useState(false);
  const [catalogs, setCatalogs] = useState<Record<string, ModelCatalog | "error">>({});
  const [, setRevision] = useState(0);
  const agentsRequest = useRef<Promise<SummaryExecutor[]> | null>(null);
  const catalogRequests = useRef(new Map<string, Promise<ModelCatalog | null>>());
  const generateOnPick = useRef(false);
  const onError = useRef(options.onError); onError.current = options.onError;
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  const loadAgents = (fresh = false) => {
    if (agentsRequest.current && !fresh) return agentsRequest.current;
    setLoadingAgents(true);
    const request = availableAgentExecutors(taskId).then(items => { if (mounted.current) setAgents(items); return items; })
      .finally(() => { if (mounted.current && agentsRequest.current === request) setLoadingAgents(false); });
    request.catch(() => { if (agentsRequest.current === request) agentsRequest.current = null; });
    agentsRequest.current = request;
    return request;
  };
  const loadCatalog = (executor: SummaryExecutor) => {
    const key = agentChoiceKey(executor), existing = catalogRequests.current.get(key);
    if (existing) return existing;
    const request = invoke<ModelCatalog>("core_available_models", { executor, selectedModel: null, taskId, refresh: false })
      .then(catalog => { if (mounted.current) setCatalogs(all => ({ ...all, [key]: catalog })); return catalog; },
        () => { catalogRequests.current.delete(key); if (mounted.current) setCatalogs(all => ({ ...all, [key]: "error" })); return null; });
    catalogRequests.current.set(key, request);
    return request;
  };

  const selected = agentFor(agents, saved, preferences, fallbackProviderId);
  const selectedKey = agentChoiceKey(selected);
  const catalog = selected ? catalogs[selectedKey] : undefined;
  const models = usableModels(catalog === "error" ? null : catalog);
  const model = selected ? modelFor(selected, catalog === "error" ? null : catalog) : null;
  const modelLabel = model ? models.find(option => option.id === model)?.label ?? model : null;

  useEffect(() => { if (enabled && fallbackProviderId !== undefined) void loadAgents().catch(cause => onError.current?.(cause)); }, [taskId, enabled]);
  useEffect(() => { if (enabled && selected && catalog === undefined) void loadCatalog(selected); }, [selectedKey, enabled]);
  useEffect(() => {
    if (!agentMenu) { generateOnPick.current = false; return; }
    void loadAgents(true).catch(cause => onError.current?.(cause));
  }, [agentMenu]);

  /** The Agent and model to use now, or null when the user must pick an Agent first. */
  const resolve = async (): Promise<SummaryChoice | null> => {
    const items = agents ?? await loadAgents();
    const executor = agentFor(items, readSummaryAgent(), readAgentPreferences(), fallbackProviderId);
    if (!executor) return null;
    return { executor, model: modelFor(executor, await loadCatalog(executor)) };
  };
  const chooseAgent = (executor: SummaryExecutor) => {
    setSaved(executor); setAgentMenuState(false);
    try { localStorage.setItem(COMMIT_AGENT_KEY, JSON.stringify(executor)); } catch { /* Selection still works for this view. */ }
    const generate = generateOnPick.current; generateOnPick.current = false;
    return generate;
  };
  const chooseModel = (id: string) => {
    if (!selected) return;
    saveSummaryModel(selected, id); setRevision(n => n + 1); setModelMenu(false);
  };
  const setAgentMenu = (open: boolean, generate = false) => {
    generateOnPick.current = open && generate;
    setModelMenu(false); setAgentMenuState(open);
  };
  const toggleModelMenu = () => {
    setAgentMenuState(false);
    if (!modelMenu && selected && catalog === "error") { setCatalogs(all => { const next = { ...all }; delete next[selectedKey]; return next; }); void loadCatalog(selected); }
    setModelMenu(!modelMenu);
  };
  const close = () => { setAgentMenuState(false); setModelMenu(false); };
  return { agents: agents ?? [], loadingAgents, selected, model, modelLabel, models, loadingModels: !!selected && catalog === undefined,
    agentMenu, modelMenu, resolve, chooseAgent, chooseModel, setAgentMenu, toggleModelMenu, close };
}
export type CommitSummaryChoiceState = ReturnType<typeof useCommitSummaryChoice>;

/** Agent button, then model button, with their menus. Picking an Agent from a
 * menu opened by Generate calls `onGenerate` with the finished choice. */
export function CommitSummaryPicker({ choice, disabled, reason, onGenerate }: {
  choice: CommitSummaryChoiceState; disabled: boolean; reason?: string;
  onGenerate?: (choice: SummaryChoice) => void;
}) {
  const root = useRef<HTMLDivElement>(null), agentTrigger = useRef<HTMLButtonElement>(null), modelTrigger = useRef<HTMLButtonElement>(null);
  const { selected, agentMenu, modelMenu } = choice;
  useEffect(() => {
    if (!agentMenu && !modelMenu) return;
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) choice.close(); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.stopPropagation(); choice.close(); (modelMenu ? modelTrigger : agentTrigger).current?.focus(); } };
    document.addEventListener("pointerdown", outside); document.addEventListener("keydown", escape, true);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape, true); };
  }, [agentMenu, modelMenu]);
  useEffect(() => { if (disabled) choice.close(); }, [disabled]);
  const agentName = selected ? providerLabel(selected.providerId) : t("gitCommit.selectAgent");
  const chevron = <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m4.5 6.5 3.5 3.5 3.5-3.5" /></svg>;
  return <div className="commit-summary-picker" ref={root}>
    <button ref={agentTrigger} type="button" className="commit-summary-choice commit-summary-agent" aria-label={t("review.selectCommitAgent")} data-tooltip={reason || t("gitCommit.agentTooltip", { agent: agentName })}
      aria-expanded={agentMenu} aria-haspopup="dialog" disabled={disabled} onClick={() => choice.setAgentMenu(!agentMenu)}>
      {selected && <AgentIcon providerId={selected.providerId} size="small"/>}
      <span>{agentName}</span>{chevron}
    </button>
    {selected && (choice.modelLabel || modelMenu) && <button ref={modelTrigger} type="button" className="commit-summary-choice commit-summary-model" aria-label={t("gitCommit.chooseModel")}
      data-tooltip={reason || t("gitCommit.modelTooltip", { model: choice.modelLabel ?? "" })} aria-expanded={modelMenu} aria-haspopup="dialog" disabled={disabled} onClick={choice.toggleModelMenu}>
      <span>{choice.modelLabel}</span>{chevron}
    </button>}
    {modelMenu && <FloatingPopover className="agent-picker-menu commit-summary-model-menu" role="dialog" aria-label={t("gitCommit.chooseModel")} placement="bottom">
      {choice.loadingModels ? <LoadingStatus>{t("agentPicker.loading")}</LoadingStatus>
        : choice.models.map(option => {
          const chosen = option.id === choice.model;
          return <button type="button" key={option.id} className={`commit-summary-model-option${chosen ? " active" : ""}`} aria-pressed={chosen} onClick={() => { choice.chooseModel(option.id); modelTrigger.current?.focus(); }}>
            <span>{option.label}</span>{chosen && <i>✓</i>}
          </button>;
        })}
    </FloatingPopover>}
    {agentMenu && <AgentPickerMenu options={choice.agents.map(executor => ({ key: agentChoiceKey(executor), executor }))}
      selectedKey={agentChoiceKey(selected)} title={t("gitCommit.chooseAgent")} label={t("gitCommit.agentListLabel")} role="dialog" placement="bottom" loading={choice.loadingAgents}
      onSelect={option => {
        if (choice.chooseAgent(option.executor) && onGenerate) void choice.resolve().then(resolved => { if (resolved) onGenerate(resolved); });
        else agentTrigger.current?.focus();
      }}/>}
  </div>;
}
