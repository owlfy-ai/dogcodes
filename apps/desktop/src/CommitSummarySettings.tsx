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
