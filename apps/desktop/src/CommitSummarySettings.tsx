export const COMMIT_AGENT_KEY = "dogcodes.git.summaryAgent";
export const COMMIT_LANGUAGE_KEY = "dogcodes.git.summaryLanguage";
export interface SummaryExecutor { providerId: string; installationId: string | null }
export const SUMMARY_LANGUAGES = [
  { id: "zh-CN", label: "简体中文" },
  { id: "zh-TW", label: "繁體中文" },
  { id: "en", label: "English" },
  { id: "ja", label: "日本語" },
  { id: "ko", label: "한국어" },
  { id: "fr", label: "Français" },
  { id: "de", label: "Deutsch" },
  { id: "es", label: "Español" },
  { id: "pt-BR", label: "Português (Brasil)" },
  { id: "ru", label: "Русский" },
  { id: "it", label: "Italiano" },
] as const;
export type SummaryLanguage = (typeof SUMMARY_LANGUAGES)[number]["id"];
export function readSummaryAgent(): SummaryExecutor | null {
  try { const value = JSON.parse(localStorage.getItem(COMMIT_AGENT_KEY) ?? "null"); return typeof value?.providerId === "string" && (typeof value?.installationId === "string" || value?.installationId === null) ? value : null; } catch { return null; }
}
// Commit languages are listed by their own names (endonyms) in every UI language.
export function summaryLanguageLabel(language: SummaryLanguage): string {
  return SUMMARY_LANGUAGES.find(option => option.id === language)?.label ?? SUMMARY_LANGUAGES[0].label;
}
export function readSummaryLanguage(): SummaryLanguage {
  try { const value = localStorage.getItem(COMMIT_LANGUAGE_KEY); return SUMMARY_LANGUAGES.some(option => option.id === value) ? value as SummaryLanguage : "zh-CN"; } catch { return "zh-CN"; }
}
export const COMMIT_MODEL_KEY = "dogcodes.git.summaryModels";
const summaryAgentKey = (executor: SummaryExecutor) => `${executor.providerId}:${executor.installationId ?? ""}`;
function readSummaryModels(): Record<string, string> {
  try { const value = JSON.parse(localStorage.getItem(COMMIT_MODEL_KEY) ?? "{}"); return value && typeof value === "object" && !Array.isArray(value) ? value : {}; } catch { return {}; }
}
/** The model chosen for one Agent's commit messages; null keeps the Agent's default. */
export function readSummaryModel(executor: SummaryExecutor | null | undefined): string | null {
  if (!executor) return null;
  const value = readSummaryModels()[summaryAgentKey(executor)];
  return typeof value === "string" && value ? value : null;
}
export function saveSummaryModel(executor: SummaryExecutor, model: string | null) {
  const all = readSummaryModels();
  if (model) all[summaryAgentKey(executor)] = model; else delete all[summaryAgentKey(executor)];
  try { localStorage.setItem(COMMIT_MODEL_KEY, JSON.stringify(all)); } catch { /* Selection still works for this panel. */ }
}
