// @vitest-environment jsdom
import { expect, it } from "vitest";
import { quotaDiagnostic } from "./App";
import { agentAccountIssue, needsAgentLogin } from "./AgentLogin";

it("distinguishes custom service, login, network and rate limiting without exposing raw errors", () => {
  expect(quotaDiagnostic("claude_quota_custom_provider")).toContain("自定义 API");
  expect(quotaDiagnostic("claude_quota_login_required")).toContain("订阅账号登录");
  expect(quotaDiagnostic("claude_quota_network_error")).toContain("系统代理");
  expect(quotaDiagnostic("claude_quota_rate_limited")).toContain("请求受限");
  expect(quotaDiagnostic("secret raw response")).not.toContain("secret");
});

it("offers login only for explicit authentication failures", () => {
  const installation = { status: "not_configured" as const };
  expect(needsAgentLogin(installation)).toBe(false);
  expect(agentAccountIssue(installation)?.label).toBe("未配置");
  expect(needsAgentLogin({ ...installation, diagnostic: "authentication_failed" })).toBe(true);
  for (const [diagnostic, label, text] of [
    ["quota_not_configured", "未配置", "运行环境"],
    ["quota_subscription_required", "未订阅或已到期", "有效订阅"],
    ["quota_access_denied", "无访问权限", "访问权限"],
  ]) {
    const view = { providerId: "codex", installationId: "ins_one", quota: null, diagnostic };
    expect(needsAgentLogin(installation, view)).toBe(false);
    expect(agentAccountIssue(installation, view)?.label).toBe(label);
    expect(quotaDiagnostic(diagnostic)).toContain(text);
  }
  expect(quotaDiagnostic("quota_login_required")).toContain("授权");
  expect(quotaDiagnostic("quota_rate_limited")).toContain("暂时受限");
});
