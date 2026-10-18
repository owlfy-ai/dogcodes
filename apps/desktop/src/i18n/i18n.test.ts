import { describe, expect, it } from "vitest";
import {
  LOCALES,
  SUPPORTED_LANGUAGES,
  resolveLanguage,
  translate,
  t,
  getPreferenceLabels,
} from "./index";
import type { ResolvedLanguage } from "./types";

describe("i18n core", () => {
  const resolvedLanguages: ResolvedLanguage[] = [
    "zh-CN",
    "zh-TW",
    "en",
    "ja",
    "ko",
    "fr",
    "de",
    "es",
    "pt-BR",
    "ru",
    "it",
  ];

  it("contains all international languages in supported languages list", () => {
    const ids = SUPPORTED_LANGUAGES.map((l) => l.id);
    expect(ids).toContain("system");
    resolvedLanguages.forEach((lang) => {
      expect(ids).toContain(lang);
    });
  });

  it("has dictionary translations for every supported language", () => {
    resolvedLanguages.forEach((lang) => {
      const dict = LOCALES[lang];
      expect(dict).toBeDefined();
      expect(dict.common.save).toBeTruthy();
      expect(dict.preferences.title).toBeTruthy();
      expect(dict.nav.newConversation).toBeTruthy();
      expect(dict.composer.placeholder).toBeTruthy();
      expect(dict.welcome.greeting).toBeTruthy();
      expect(dict.terminal.title).toBeTruthy();
    });
  });

  it("resolves system language correctly", () => {
    expect(resolveLanguage("zh-CN")).toBe("zh-CN");
    expect(resolveLanguage("en")).toBe("en");
    expect(resolveLanguage("ja")).toBe("ja");
    expect(resolveLanguage("system", "zh-TW")).toBe("zh-TW");
    expect(resolveLanguage("system", "zh-HK")).toBe("zh-TW");
    expect(resolveLanguage("system", "zh-CN")).toBe("zh-CN");
    expect(resolveLanguage("system", "ja-JP")).toBe("ja");
    expect(resolveLanguage("system", "ko-KR")).toBe("ko");
    expect(resolveLanguage("system", "fr-FR")).toBe("fr");
    expect(resolveLanguage("system", "de-DE")).toBe("de");
    expect(resolveLanguage("system", "es-ES")).toBe("es");
    expect(resolveLanguage("system", "pt-BR")).toBe("pt-BR");
    expect(resolveLanguage("system", "ru-RU")).toBe("ru");
    expect(resolveLanguage("system", "it-IT")).toBe("it");
    expect(resolveLanguage("system", "en-US")).toBe("en");
  });

  it("interpolates parameters in translation strings", () => {
    const zh = translate("welcome.readyAgentSelected", { agent: "Codex" }, "zh-CN");
    expect(zh).toContain("Codex");
    const enText = translate("welcome.readyAgentSelected", { agent: "Codex" }, "en");
    expect(enText).toContain("Codex");
  });

  it("provides preferences labels correctly", () => {
    const zhLabels = getPreferenceLabels("zh-CN");
    expect(zhLabels.language).toBe("界面语言");
    const enLabels = getPreferenceLabels("en");
    expect(enLabels.language).toBe("Language");
    const jaLabels = getPreferenceLabels("ja");
    expect(jaLabels.language).toBe("表示言語");
  });
});
