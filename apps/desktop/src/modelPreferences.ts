import type { ModelOption } from "@dogcodes/protocol";

/** The model (and thinking effort) last picked per Agent, keyed by providerId.
 *  Only seeds new conversations; existing sessions keep their own saved model. */
export const MODEL_PREFERENCES_KEY = "dogcodes.agents.models.v1";
export interface ModelChoice { model: string; thinking: string | null }
type Stored = Record<string, ModelChoice>;
// Used when storage is unavailable so a choice still lasts for this window.
let memory: Stored = {};
let memoryOnly = false;

function readAll(): Stored {
  if (memoryOnly) return memory;
  let raw: string | null;
  try { raw = localStorage.getItem(MODEL_PREFERENCES_KEY); } catch { return memory; }
  if (raw === null) return {};
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    const result: Stored = {};
    for (const [providerId, choice] of Object.entries(value as Record<string, unknown>)) {
      const { model, thinking } = (choice ?? {}) as { model?: unknown; thinking?: unknown };
      if (typeof model === "string" && model) result[providerId] = { model, thinking: typeof thinking === "string" ? thinking : null };
    }
    return result;
  } catch { return {}; }
}

export function readRememberedModel(providerId: string): ModelChoice | null {
  return readAll()[providerId] ?? null;
}

export function rememberModel(providerId: string, model: string | null, thinking: string | null) {
  if (!providerId || providerId === "fake" || !model) return;
  const next = { ...readAll(), [providerId]: { model, thinking } };
  memory = next;
  try { localStorage.setItem(MODEL_PREFERENCES_KEY, JSON.stringify(next)); memoryOnly = false; } catch { memoryOnly = true; }
}

/** The starting model for a new conversation: the remembered one when the Agent still offers it,
 *  otherwise the catalog's first model (the existing default). */
export function initialModelChoice(providerId: string, models: readonly ModelOption[]): ModelChoice | null {
  const saved = readRememberedModel(providerId);
  const option = saved && models.find(item => item.id === saved.model);
  if (saved && option) {
    const thinking = saved.thinking && option.thinkingOptions.some(item => item.id === saved.thinking) ? saved.thinking : option.defaultThinking ?? null;
    return { model: option.id, thinking };
  }
  const first = models[0];
  return first ? { model: first.id, thinking: first.defaultThinking ?? null } : null;
}
