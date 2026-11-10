import { getGlobalResolvedLanguage, translate, type ResolvedLanguage } from "./i18n";
import { backendMessages } from "./backendTextCatalog";

// Backend services (core, store, drivers) persist and return fixed Chinese
// messages. The Chinese text stays as the stored/diagnostic form; known
// messages are translated here for display.
/** Prompt the backend sends for review-fix turns (REPAIR_PROMPT in review_repairs.rs); shown as a localized label. */
export const REVIEW_REPAIR_PROMPT = "Please handle the review findings I selected.";
const exact =new Map(backendMessages);
const patterns = backendMessages.filter(([zh]) => zh.includes("{")).map(([zh, key]) => {
  const names: string[] = [];
  const source = zh.split(/\{(\w+)\}/).map((part, index) => index % 2
    ? (names.push(part), "([\\s\\S]+?)")
    : part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("");
  return { key, names, regex: new RegExp(`^${source}$`) };
});

/** Translation of a known backend message, or null when the text is not a known one. */
export function localizeBackendText(text: string, lang: ResolvedLanguage = getGlobalResolvedLanguage()): string | null {
  const key = exact.get(text);
  if (key) return translate(`backendText.${key}`, undefined, lang);
  for (const { key: patternKey, names, regex } of patterns) {
    const match = regex.exec(text);
    if (!match) continue;
    const params: Record<string, string> = {};
    names.forEach((name, index) => { params[name] = match[index + 1]; });
    // Translate known nested errors and recovery guidance; preserve unknown details.
    for (const name of ["message", "recovery"]) {
      if (params[name] !== undefined) params[name] = localizeBackendText(params[name], lang) ?? params[name];
    }
    return translate(`backendText.${patternKey}`, params, lang);
  }
  return null;
}

/** Display text for a backend-provided message: translated when known, otherwise unchanged. */
export function displayBackendText(text: string, lang: ResolvedLanguage = getGlobalResolvedLanguage()): string {
  return lang === "zh-CN" ? text : localizeBackendText(text, lang) ?? text;
}
