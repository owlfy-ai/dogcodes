import { beforeAll, describe, expect, it, vi } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { AgentQuotaRing } from "./AgentQuotaRing";
import type { QuotaSnapshotView } from "@dogcodes/protocol";

let components: typeof import("./App");
beforeAll(async () => {
  vi.stubGlobal("window", {});
  components = await import("./App");
  vi.unstubAllGlobals();
});

const quota: NonNullable<QuotaSnapshotView["quota"]> = {
  windows: [{ id: "weekly", windowMinutes: 10080, usedPercent: null, resetsAtUnix: 1800000000 }],
  context: { usedTokens: 1200, windowTokens: 200000, cacheHitRatePercent: 80 },
  collectedAtUnixMs: 1000, staleAfterUnixMs: 2000,
  planType: null, source: "native_pull", confidence: "provider_reported", parserVersion: "test",
};

describe("quota components", () => {
  it("separates configuration from authentication and keeps diagnostics alongside session-only data", () => {
    const configuration = renderToStaticMarkup(createElement(components.QuotaPanel, {
      management: true,
      views: [{ providerId: "zcode", installationId: "ins_zcode", quota: null }],
      installations: [{ providerId: "zcode", installationId: "ins_zcode", status: "not_configured", version: "1.0" }],
    }));
    expect(configuration).toContain("未配置");
    expect(configuration).not.toContain("去登录");
    const authentication = renderToStaticMarkup(createElement(components.QuotaPanel, {
      management: true,
      views: [{ providerId: "kimi", installationId: "ins_kimi", quota: { ...quota, windows: [] }, diagnostic: "quota_login_required" }],
      installations: [{ providerId: "kimi", installationId: "ins_kimi", status: "ready", version: "1.0" }],
    }));
    expect(authentication).toContain("未登录");
    expect(authentication).toContain("去登录");
    expect(authentication).toContain("授权缺失、失效或被拒绝");
    expect(authentication).toContain("当前会话上下文");
  });
  it("labels the Cursor overview pool and displays both separate allowances", () => {
    const split = { ...quota, context: null, planType: "Pro · 重置 Oct 4（原生日期）", staleAfterUnixMs: Date.now() + 60_000,
      windows: [
        { id: "cursor-models", scope: "Cursor Models", windowMinutes: 0, usedPercent: 20 },
        { id: "cursor-other-models", scope: "Other Models", windowMinutes: 0, usedPercent: 100 },
      ] };
    const ring = renderToStaticMarkup(createElement(components.QuotaRing, { quota: split, providerId: "cursor" }));
    expect(ring).toContain('aria-label="Cursor Models · 每月剩余额度 80%"');
    expect(ring).toContain("Other Models · 每月");
    expect(ring).toContain("圆环：Cursor Models · 每月");
    expect(ring).not.toContain("Coding Agent 额度已用尽");
    const picker = renderToStaticMarkup(createElement(AgentQuotaRing, { quota: split, providerId: "cursor" }));
    expect(picker).toContain("Cursor Models · 每月剩余额度 80%");
    const panel = renderToStaticMarkup(createElement(components.QuotaPanel, {
      views: [{ providerId: "cursor", installationId: "ins_cursor", quota: split }],
      installations: [{ installationId: "ins_cursor", version: "2026.09.23" }],
    }));
    for (const label of ["Cursor Models", "Other Models", "80%", "已用尽", "10/4"]) expect(panel).toContain(label);
  });
  it("does not show an expired balance as a current progress value", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaPanel, {
      views: [{ providerId: "codex", installationId: "ins_test", quota: { ...quota,
        windows: [{ id: "5h", windowMinutes: 300, usedPercent: 0, resetsAtUnix: Date.now() / 1000 - 60 }] } }],
      installations: [{ installationId: "ins_test", version: "1.0" }],
    }));
    expect(html).toContain("已到重置时间，待刷新");
    expect(html).not.toContain('aria-valuenow="100"');
    expect(html).not.toContain('role="progressbar"');
  });

  it("separates the desktop version from its bundled engine even if update checks fail", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaPanel, {
      views: [{ providerId: "codex", installationId: "ins_new", quota }],
      installations: [{ installationId: "ins_new", version: "0.155.0-alpha.16.4" }],
      agentUpdates: { updates: { ins_new: {
        installationId: "ins_new", status: "error", currentVersion: "0.155.0-alpha.16.4",
        desktopVersion: "26.917.71314", latestVersion: null, source: "", message: "网络不可用",
        canUpgrade: false, checkedAt: 0,
      } }, check: vi.fn(), upgrade: vi.fn() },
    }));
    expect(html).toContain("客户端 26.917.71314 · 引擎 0.155.0-alpha.16.4");
  });
  it("renders only the selected Codex installation even when obsolete readings arrive", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaPanel, {
      views: [
        { providerId: "codex", installationId: "ins_old", quota },
        { providerId: "codex", installationId: "ins_new", quota },
      ],
      installations: [
        { installationId: "ins_new", version: "0.153.4" },
      ],
    }));
    expect(html.match(/<section/g)).toHaveLength(1);
    expect(html).toContain("0.153.4");
    expect(html).not.toContain("0.128.0");
  });

  it("renders the percentage after the ring instead of tiny SVG text", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaRing, {
      quota: { ...quota, staleAfterUnixMs: Date.now() + 60_000,
        windows: [{ id: "5h", windowMinutes: 300, usedPercent: 2 }] },
    }));
    expect(html).toContain('</svg><span class="quota-ring-value" aria-hidden="true">98%</span>');
    expect(html).not.toContain("<text");
  });
  it("shows a reset bucket as pending refresh, not a current zero balance", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaRing, {
      quota: { ...quota, staleAfterUnixMs: Date.now() + 60_000,
        windows: [{ id: "5h", windowMinutes: 300, usedPercent: 100, resetsAtUnix: Date.now() / 1000 - 1 }] },
    }));
    expect(html).toContain("已到重置时间，等待读取新额度");
    expect(html).toContain("额度暂无读数");
    expect(html).not.toContain("Coding Agent 额度已用尽");
  });

  it("does not display a tiny positive balance as zero or exhausted", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaRing, {
      quota: { ...quota, staleAfterUnixMs: Date.now() + 60_000,
        windows: [{ id: "5h", windowMinutes: 300, usedPercent: 99.96 }] },
    }));
    expect(html).toContain("剩余额度 0.1%");
    expect(html).not.toContain("Coding Agent 额度已用尽");
  });
  it("keeps unknown quota details and reset time accessible", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaRing, { quota }));
    expect(html).toContain("用量未知");
    expect(html).toContain("重置");
    expect(html).not.toContain("数据待刷新");
    expect(html).toContain("缓存命中");
    expect(html).toContain('tabindex="0"');
  });

  it("shows context-only and no-observation states without inventing a percent", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaRing, { quota: { ...quota, windows: [] } }));
    expect(html).toContain("暂无订阅额度读数");
    expect(html).toContain("上下文");
    const empty = renderToStaticMarkup(createElement(components.QuotaRing, { quota: null }));
    expect(empty).toContain("额度暂无读数");
  });

  it("shows context-only installations without a periodically changing stale banner", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaPanel, {
      views: [{ providerId: "claude-code", installationId: "ins_test", quota: { ...quota, windows: [] } }],
      installations: [{ installationId: "ins_test", version: "2.1.266" }],
    }));
    expect(html).toContain("当前会话上下文");
    expect(html).not.toContain("数据待刷新");
    expect(html).toContain("2.1.266");
    expect(html).not.toContain("ins_test");
    expect(html).toContain("更新于");
    expect(html).not.toContain("暂无官方额度数据源");
  });

  it("does not let unrelated model pools clear a real turn exhaustion", () => {
    const html = renderToStaticMarkup(createElement(components.QuotaRing, {
      quota: { ...quota, staleAfterUnixMs: Date.now() + 60_000,
        windows: [{ id: "gemini", scope: "Gemini Models", windowMinutes: 300, usedPercent: 0 }] },
      turnQuotaExhausted: true,
    }));
    expect(html).toContain("额度已用尽");
    expect(html).toContain("Gemini");
    expect(html).toContain("当前模型组未确定");
  });

  it("renders an explicit exhausted state from a fresh window or a turn failure", () => {
    const freshExhausted = {
      ...quota,
      windows: [{ id: "primary", windowMinutes: 300, usedPercent: 100, resetsAtUnix: 1_800_000_000 }],
      staleAfterUnixMs: Date.now() + 60_000,
    };
    const ring = renderToStaticMarkup(createElement(components.QuotaRing, { quota: freshExhausted }));
    expect(ring).toContain("额度已用尽");
    expect(ring).toContain(">!<");

    const turnFailure = renderToStaticMarkup(createElement(components.QuotaRing, {
      quota: null,
      turnQuotaExhausted: true,
    }));
    expect(turnFailure).toContain("额度已用尽");

    const recovered = renderToStaticMarkup(createElement(components.QuotaRing, {
      quota: {
        ...freshExhausted,
        windows: [{ ...freshExhausted.windows[0], usedPercent: 20 }],
      },
      turnQuotaExhausted: true,
    }));
    expect(recovered).not.toContain("额度已用尽。请等待");

    const panel = renderToStaticMarkup(createElement(components.QuotaPanel, {
      views: [{ providerId: "codex", installationId: "ins_test", quota: freshExhausted }],
      installations: [{ installationId: "ins_test", version: "1.0" }],
    }));
    expect(panel).toContain("已用尽");

    const footer = renderToStaticMarkup(createElement(components.ComposerFooter, {
      selectedTask: {
        taskId: "tsk_test",
        projectId: "prj_test",
        title: "test",
        preferredExecutor: { providerId: "codex", installationId: "ins_test" },
        activePrimaryRun: null,
        streamVersion: 1,
      },
      selectedQuota: freshExhausted,
      turnQuotaExhausted: false,
      executorOptions: [{
        key: "codex:ins_test",
        executor: { providerId: "codex", installationId: "ins_test" },
        quota: freshExhausted,
      }],
      selectedInstallationStatus: "ready",
      sending: false,
      cancelling: false,
      draft: "hello",
      onSelect: vi.fn(),
      onCancel: vi.fn(),
    }));
    expect(footer).toContain("额度已用尽，暂时无法发送");
    expect(footer).toContain("额度已用尽");
    // Check visible labels, not coincidental numbers inside the Agent SVG path.
    expect(footer.replace(/<[^>]*>/g, "")).not.toContain("1.0");
    expect(footer).toContain("disabled");
  });
});
