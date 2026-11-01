import { describe, expect, it } from "vitest";
import { displayBackendText, localizeBackendText } from "./backendText";
import { backendMessages } from "./backendTextCatalog";
import { setGlobalResolvedLanguage } from "./i18n";
import { userFacingError } from "./userFacingError";

describe("backend text localization", () => {
  it("translates every catalog message and keeps zh-CN verbatim", () => {
    for (const [zh] of backendMessages.filter(([text]) => !text.includes("{"))) {
      expect(localizeBackendText(zh, "en")).not.toMatch(/\p{Script=Han}/u);
      expect(displayBackendText(zh, "zh-CN")).toBe(zh);
    }
  });

  it("translates parameterized and agent-error-wrapped messages", () => {
    expect(localizeBackendText("Coding Agent 额度已用尽；预计于 2026-01-02 03:04 UTC 重置，也可以切换 Agent。", "en")).toContain("2026-01-02 03:04 UTC");
    const wrapped = localizeBackendText("Agent 请求暂时受限，请稍后重试。\n\nAgent 返回的错误：\n429 too many", "en");
    expect(wrapped).toContain("rate limited");
    expect(wrapped).toContain("429 too many");
  });

  it("never leaks unmapped Chinese through userFacingError outside zh-CN", () => {
    setGlobalResolvedLanguage("en");
    try {
      expect(userFacingError("某个没有映射的后端错误", "fallback")).toBe("fallback");
      expect(userFacingError("附件已不可读取", "fallback")).toBe("The attachment can no longer be read.");
    } finally {
      setGlobalResolvedLanguage("zh-CN");
    }
    expect(userFacingError("某个没有映射的后端错误", "fallback")).toBe("某个没有映射的后端错误");
  });
});
