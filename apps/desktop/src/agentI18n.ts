import { t, type ResolvedLanguage } from "./i18n";
import { agentUiZh, type AgentUiMessages } from "./i18n/agentUi";
import { userFacingError } from "./userFacingError";
import { localizeBackendText } from "./backendText";

// Older desktop services return display messages. Keep this compatibility at
// the presentation boundary; account diagnostics and update status remain data.
const nativeMessages = new Map(Object.entries(agentUiZh).map(([key, value]) => [value, key]));

export function localizedAgentMessage(message: string | null | undefined, fallback: keyof AgentUiMessages, lang: ResolvedLanguage): string {
  if (!message) return t(`agentUi.${fallback}`, undefined, lang);
  const translated = localizeBackendText(message, lang);
  if (translated) return translated;
  const key = message.startsWith("agentUi.") ? message : nativeMessages.has(message) ? `agentUi.${nativeMessages.get(message)}` : null;
  if (key) return t(key, undefined, lang);
  // Do not expose an untranslated Chinese service message in another language.
  // The localized status still explains the action available to the user.
  return lang !== "zh-CN" && /\p{Script=Han}/u.test(message) ? t(`agentUi.${fallback}`, undefined, lang) : message;
}

export function localizedUpdateSource(source: string, lang: ResolvedLanguage): string {
  // Product names and URLs stay verbatim. Service-owned labels are `{agentUi.*}` tokens.
  return source.replace(/\{agentUi\.([A-Za-z0-9]+)\}/g, (_match, key: string) => t(`agentUi.${key}`, undefined, lang));
}

export function localizedAgentError(cause: unknown, fallback: string, lang: ResolvedLanguage): string {
  if (typeof cause === "string" && cause.startsWith("agentUi.")) return t(cause, undefined, lang);
  const detail = userFacingError(cause, t(fallback, undefined, lang));
  const key = nativeMessages.get(detail);
  if (key) return t(`agentUi.${key}`, undefined, lang);
  return lang !== "zh-CN" && /\p{Script=Han}/u.test(detail) ? t(fallback, undefined, lang) : detail;
}
