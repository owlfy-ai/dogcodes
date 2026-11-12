import { getGlobalResolvedLanguage, translate, type ResolvedLanguage } from "./i18n";

/** Prompt the backend sends for review-fix turns (REPAIR_PROMPT in review_repairs.rs); shown as a localized label. */
export const REVIEW_REPAIR_PROMPT = "Please handle the review findings I selected.";

// Parameter names for codes that carry a JSON string array after U+001E.
const codeParams: Record<string, string[]> = {
  "backendText.undoFileChanged": ["relative"],
  "backendText.undoIncomplete": ["message", "recovery"],
  "backendText.installNodeTooOld": ["current"],
  "backendText.installFailedCode": ["code"],
  "backendText.codexAppUpgrade": ["current", "latest"],
  "backendText.codexAppLatest": ["current"],
  "backendText.failureWithAgentError": ["detail"],
  "backendText.failureAgentErrorSuffix": ["message", "detail"],
  "backendText.agentRunsOneTaskAtATime": ["agent"],
  "backendText.quotaExhaustedReset": ["time"],
  "backendText.zcodeAppUpgrade": ["latest"],
  "backendText.updateCommandFailed": ["code"],
  "backendText.missingSymbol": ["name"],
  "phone.connectionFailed": ["detail"],
  "phone.httpError": ["code"],
  "shell.exitUnverified": ["error"],
  "shell.exitSaveFailed": ["error"],
  "project.analysisNotSaved": ["error"],
};
const codePattern = /^[a-z][A-Za-z0-9]*(?:\.[A-Za-z][A-Za-z0-9]*)+$/;

/** Translation of a backend message code, or null when the text is not one. */
export function localizeBackendText(text: string, lang: ResolvedLanguage = getGlobalResolvedLanguage()): string | null {
  const separator = text.indexOf("\u001e");
  const code = separator < 0 ? text : text.slice(0, separator);
  if (!codePattern.test(code)) return null;
  let values: string[] = [];
  if (separator >= 0) {
    try {
      const parsed = JSON.parse(text.slice(separator + 1)) as unknown;
      if (!Array.isArray(parsed) || parsed.some(value => typeof value !== "string")) return null;
      values = parsed;
    } catch {
      return null;
    }
  }
  const params: Record<string, string> = {};
  const names = codeParams[code]?.length ? codeParams[code] : values.map((_, index) => String(index));
  for (const [index, name] of names.entries()) {
    const value = values[index] ?? "";
    const nested = codePattern.test(value.split("\u001e")[0] ?? "") ? localizeBackendText(value, lang) : null;
    params[name] = nested ?? localizeEmbedded(value, lang);
  }
  const translated = translate(code, Object.keys(params).length ? params : undefined, lang);
  return translated === code ? null : translated;
}

/** A code sitting inside a larger error, such as a redacted URL or a driver prefix. */
function localizeEmbedded(text: string, lang: ResolvedLanguage): string {
  return text.replace(/backendText\.[A-Za-z0-9]+/g, (token: string, offset: number) => {
    if (text[offset + token.length] === "\u001e") return token;
    return localizeBackendText(token, lang) ?? token;
  });
}

/** Display text for a backend message code. Unknown text is left unchanged. */
export function displayBackendText(text: string, lang: ResolvedLanguage = getGlobalResolvedLanguage()): string {
  return localizeBackendText(text, lang) ?? localizeEmbedded(text, lang);
}
