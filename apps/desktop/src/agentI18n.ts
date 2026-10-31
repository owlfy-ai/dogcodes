import { t, type ResolvedLanguage } from "./i18n";
import { agentUiZh, type AgentUiMessages } from "./i18n/agentUi";
import { userFacingError } from "./userFacingError";

// Older desktop services return display messages. Keep this compatibility at
// the presentation boundary; account diagnostics and update status remain data.
const nativeMessages = new Map(Object.entries(agentUiZh).map(([key, value]) => [value, key]));
nativeMessages.set("桌面应用已是最新版本", "latest");

export function localizedAgentMessage(message: string | null | undefined, fallback: keyof AgentUiMessages, lang: ResolvedLanguage): string {
  if (!message) return t(`agentUi.${fallback}`, undefined, lang);
  const key = message.startsWith("agentUi.") ? message : nativeMessages.has(message) ? `agentUi.${nativeMessages.get(message)}` : null;
  if (key) return t(key, undefined, lang);
  const hostVersion = /^桌面应用可升级至 ([^，]+)，请在 ZCode 中完成升级$/.exec(message);
  if (hostVersion) return `${t("agentUi.upgradeTo", { version: hostVersion[1] }, lang)} · ${t("agentUi.hostUpdate", undefined, lang)}`;
  // Do not expose an untranslated Chinese service message in another language.
  // The localized status still explains the action available to the user.
  return lang !== "zh-CN" && /\p{Script=Han}/u.test(message) ? t(`agentUi.${fallback}`, undefined, lang) : message;
}

export function localizedUpdateSource(source: string, lang: ResolvedLanguage): string {
  if (lang === "zh-CN" || !/\p{Script=Han}/u.test(source)) return source;
  // Source fields also contain machine-readable package names and URLs; retain
  // those verbatim, replacing only service-owned display labels.
  return source.replace(/桌面应用/g, t("agentUi.desktopApp", undefined, lang))
    .replace(/官方更新源/g, t("agentUi.officialSource", undefined, lang))
    .replace(/外部安装/g, t("agentUi.externalInstall", undefined, lang))
    .replace(/原生更新器/g, t("agentUi.nativeUpdater", undefined, lang))
    .replace(/（当前 npx 安装）/g, " (npx)");
}

export function localizedAgentError(cause: unknown, fallback: string, lang: ResolvedLanguage): string {
  if (typeof cause === "string" && cause.startsWith("agentUi.")) return t(cause, undefined, lang);
  const detail = userFacingError(cause, t(fallback, undefined, lang));
  const key = nativeMessages.get(detail);
  if (key) return t(`agentUi.${key}`, undefined, lang);
  return lang !== "zh-CN" && /\p{Script=Han}/u.test(detail) ? t(fallback, undefined, lang) : detail;
}
