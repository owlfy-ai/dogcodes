import React, {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import type { LanguageInfo, ResolvedLanguage, TranslationDict, UiLanguage } from "./types";
import { SUPPORTED_LANGUAGES, resolveLanguage } from "./languages";
import { zhCN } from "./locales/zh-CN";
import { zhTW } from "./locales/zh-TW";
import { en } from "./locales/en";
import { ja } from "./locales/ja";
import { ko } from "./locales/ko";
import { fr } from "./locales/fr";
import { de } from "./locales/de";
import { es } from "./locales/es";
import { ptBR } from "./locales/pt-BR";
import { ru } from "./locales/ru";
import { it } from "./locales/it";

export * from "./types";
export * from "./languages";

export const LOCALES: Record<ResolvedLanguage, TranslationDict> = {
  "zh-CN": zhCN,
  "zh-TW": zhTW,
  en,
  ja,
  ko,
  fr,
  de,
  es,
  "pt-BR": ptBR,
  ru,
  it,
};

export const UI_LANGUAGE_KEY = "dogcodes.ui.language";

function getInitialLanguage(): UiLanguage {
  if (typeof window !== "undefined" && typeof window.localStorage !== "undefined" && window.localStorage) {
    try {
      const stored = window.localStorage.getItem(UI_LANGUAGE_KEY);
      if (stored) return stored as UiLanguage;
    } catch {
      // Local storage inaccessible
    }
  }
  return "system";
}

let globalResolvedLanguage: ResolvedLanguage = resolveLanguage(getInitialLanguage());

export function getGlobalResolvedLanguage(): ResolvedLanguage {
  return globalResolvedLanguage;
}

export function setGlobalResolvedLanguage(lang: ResolvedLanguage): void {
  globalResolvedLanguage = lang;
}

/**
 * Access a nested translation string by path (e.g. "nav.newConversation" or "common.save").
 * Falls back to English and then Simplified Chinese if missing.
 */
export function translate(
  keyPath: string,
  params?: Record<string, string | number>,
  resolvedLang?: ResolvedLanguage
): string {
  const currentLang = resolvedLang ?? globalResolvedLanguage;
  const dict = LOCALES[currentLang] || LOCALES["zh-CN"];

  const keys = keyPath.split(".");
  let current: any = dict;
  for (const key of keys) {
    if (current && typeof current === "object" && key in current) {
      current = current[key];
    } else {
      current = undefined;
      break;
    }
  }

  // Fallback to en, then zh-CN
  if (typeof current !== "string") {
    let fallback: any = LOCALES["en"];
    for (const key of keys) {
      if (fallback && typeof fallback === "object" && key in fallback) {
        fallback = fallback[key];
      } else {
        fallback = undefined;
        break;
      }
    }
    if (typeof fallback === "string") {
      current = fallback;
    } else {
      let zhFallback: any = LOCALES["zh-CN"];
      for (const key of keys) {
        if (zhFallback && typeof zhFallback === "object" && key in zhFallback) {
          zhFallback = zhFallback[key];
        } else {
          zhFallback = undefined;
          break;
        }
      }
      current = typeof zhFallback === "string" ? zhFallback : keyPath;
    }
  }

  if (params && typeof current === "string") {
    return current.replace(/\{(\w+)\}/g, (_, match) => {
      return match in params ? String(params[match]) : `{${match}}`;
    });
  }

  return current;
}

export const t = translate;

export interface I18nContextValue {
  language: UiLanguage;
  resolvedLanguage: ResolvedLanguage;
  setLanguage: (lang: UiLanguage) => void;
  t: (keyPath: string, params?: Record<string, string | number>) => string;
  supportedLanguages: readonly LanguageInfo[];
}

export const I18nContext = createContext<I18nContextValue>({
  language: "system",
  resolvedLanguage: "zh-CN",
  setLanguage: () => {},
  t: translate,
  supportedLanguages: SUPPORTED_LANGUAGES,
});

export function useI18n(): I18nContextValue {
  return useContext(I18nContext);
}

export function I18nProvider({
  children,
  initialLanguage,
  language: controlledLanguage,
}: {
  children: ReactNode;
  initialLanguage?: UiLanguage;
  language?: UiLanguage;
}) {
  const [localLanguage, setLanguageState] = useState<UiLanguage>(() => {
    if (initialLanguage) return initialLanguage;
    return getInitialLanguage();
  });
  const language = controlledLanguage ?? localLanguage;

  const resolvedLanguage = useMemo(() => resolveLanguage(language), [language]);
  // Helpers outside React read the global language while children render, so
  // switch it before they render rather than in an effect afterwards.
  setGlobalResolvedLanguage(resolvedLanguage);

  useEffect(() => {
    if (typeof document !== "undefined") {
      document.documentElement.lang = resolvedLanguage;
    }
  }, [resolvedLanguage]);

  const setLanguage = useCallback((next: UiLanguage) => {
    setLanguageState(next);
    if (typeof window !== "undefined" && typeof window.localStorage !== "undefined" && window.localStorage) {
      try {
        window.localStorage.setItem(UI_LANGUAGE_KEY, next);
      } catch {
        // Storage can be restricted
      }
    }
  }, []);

  const tBound = useCallback(
    (keyPath: string, params?: Record<string, string | number>) => {
      return translate(keyPath, params, resolvedLanguage);
    },
    [resolvedLanguage]
  );

  const value = useMemo(
    () => ({
      language,
      resolvedLanguage,
      setLanguage,
      t: tBound,
      supportedLanguages: SUPPORTED_LANGUAGES,
    }),
    [language, resolvedLanguage, setLanguage, tBound]
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function getPreferenceLabels(
  language: UiLanguage
): TranslationDict["preferences"] & { light: string; dark: string } {
  const resolved = resolveLanguage(language);
  const locale = LOCALES[resolved] ?? LOCALES["zh-CN"];
  return {
    ...locale.preferences,
    light: locale.common.light,
    dark: locale.common.dark,
  };
}
