// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CoreClient, InstallationSummary, QuotaSnapshot, BootstrapSnapshot } from "@dogcodes/protocol";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { AgentWelcome, supportedAgents, welcomeStatus } from "./AgentWelcome";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn(() => false) }));
// These tests advance account timers by minutes; decorative animation frames
// should not execute thousands of DOM updates while checking detection state.
vi.mock("./WelcomeHerd", () => ({ WelcomeHerd: () => null }));

const installation: InstallationSummary = { installationId: "ins_one", providerId: "codex", executable: "/bin/codex", fingerprint: "f", status: "ready", version: "1.0", versionText: null, diagnostic: null, probeElapsedMillis: 1, probedAt: "2026-09-13" };
const quota = (): QuotaSnapshot => ({ windows: [{ id: "5h", windowMinutes: 300, usedPercent: 25, resetsAtUnix: Math.floor(Date.now() / 1000) + 3600 }], source: "native_pull", confidence: "provider_reported", parserVersion: "test", collectedAtUnixMs: Date.now(), staleAfterUnixMs: Date.now() + 60_000 });
const view = (q = quota()) => ({ providerId: "codex", installationId: "ins_one", quota: q });
let root: Root | undefined;
let container: HTMLDivElement;
afterEach(async () => { if (root) await act(async () => root?.unmount()); root = undefined; container?.remove(); vi.useRealTimers(); vi.mocked(isTauri).mockReturnValue(false); vi.mocked(invoke).mockReset(); vi.unstubAllGlobals(); });

describe("welcome account states", () => {
  it("lists the twelve supported products", () => expect(supportedAgents).toHaveLength(12));
  it("does not mistake network failure, stale quota or a ready handshake for valid credentials", () => {
    expect(welcomeStatus({ installation, failed: true }).label).toBe("检测失败");
    expect(welcomeStatus({ installation }).label).toBe("授权待确认");
    expect(welcomeStatus({ installation, view: view({ ...quota(), staleAfterUnixMs: 1 }) }).label).toBe("授权待确认");
  });
  it("distinguishes a quota timeout from a missing login", () => {
    expect(welcomeStatus({ installation, view: { ...view(), quota: null, diagnostic: "quota_timeout" } }).label).toBe("额度查询超时");
    expect(welcomeStatus({ installation, view: { ...view(), quota: null, diagnostic: "quota_network_error" } }).label).toBe("额度查询网络异常");
  });
  it.each([
    ["quota_exhausted", "额度已用尽"],
    ["quota_rate_limited", "额度查询受限"],
    ["claude_quota_rate_limited", "额度查询受限"],
  ])("explains %s without suggesting an authorization failure", (diagnostic, label) => {
    const status = welcomeStatus({ installation, view: { ...view(), quota: null, diagnostic } });
    expect(status.label).toBe(label);
    expect(status.detail).not.toContain("登录");
    expect(status.detail).not.toContain("授权");
  });
  it("separates missing installation, incomplete configuration and explicit missing login", () => {
    expect(welcomeStatus().label).toBe("未安装");
    expect(welcomeStatus({ installation: { ...installation, status: "not_configured" } }).label).toBe("未配置");
    expect(welcomeStatus({ installation: { ...installation, status: "not_configured", diagnostic: "authentication_failed" } }).label).toBe("未登录");
    expect(welcomeStatus({ installation, view: { ...view(), quota: null, diagnostic: "quota_login_required" } }).label).toBe("未登录");
  });
  it("keeps configuration, subscription and access failures separate from login", () => {
    for (const [diagnostic, label] of [["quota_not_configured", "未配置"], ["quota_subscription_required", "未订阅或已到期"], ["quota_access_denied", "无访问权限"]]) {
      const status = welcomeStatus({ installation, view: { ...view(), quota: null, diagnostic } });
      expect(status.label).toBe(label);
      expect(status.detail).not.toContain("登录");
    }
  });
  it("requires fresh provider readings, while zero quota still proves login", () => {
    expect(welcomeStatus({ installation, view: view() }).label).toBe("已登录");
    const q = quota(); q.windows[0].usedPercent = 100;
    expect(welcomeStatus({ installation, view: view(q) }).label).toBe("已登录");
    expect(welcomeStatus({ installation, view: view({ ...q, confidence: "demo" }) }).label).toBe("演示数据");
  });
  it("does not claim CodeBuddy has quota or custom Claude has a subscription login", () => {
    expect(welcomeStatus({ installation: { ...installation, providerId: "codebuddy" } }).detail).toContain("暂不支持额度查询");
    expect(welcomeStatus({ installation, view: { ...view(), quota: null, diagnostic: "claude_quota_custom_provider" } }).label).toBe("第三方模型");
  });
});

async function mount(client: CoreClient) {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
  await act(async () => root!.render(<AgentWelcome client={client} onContinue={() => {}} renderIcon={() => null} />));
}
describe("welcome scan workflow", () => {
  it("waits for Antigravity cold quota startup and then confirms login", async () => {
    vi.useFakeTimers();
    const anti = { ...installation, providerId: "antigravity" };
    await mount({
      scanAgent: async (id: string) => ({ installations: id === "antigravity" ? [anti] : [] }),
      quotaSnapshots: () => new Promise(resolve => setTimeout(() => resolve([{ ...view(), providerId: "antigravity" }]), 40_000)),
    } as unknown as CoreClient);
    await act(async () => { await vi.advanceTimersByTimeAsync(36_000); });
    expect(container.querySelector('[aria-label="Antigravity"]')?.textContent).toContain("验证授权中");
    await act(async () => { await vi.advanceTimersByTimeAsync(4_000); });
    expect(container.querySelector('[aria-label="Antigravity"]')?.textContent).toContain("已登录");
  });

  it("puts installed agents first even before login and disables only confirmed missing agents", async () => {
    await mount({ scanAgent: async (id: string) => {
      if (id === "grok") throw new Error("probe failed");
      return { installations: ["kiro", "pi"].includes(id) ? [{ ...installation, providerId: id, installationId: id, status: "not_configured" }] : [] };
    }, quotaSnapshots: vi.fn() } as unknown as CoreClient);
    const cards = [...container.querySelectorAll(".welcome-agent-card")];
    expect(cards.slice(0, 3).map(card => card.getAttribute("aria-label"))).toEqual(["Pi", "Kiro", "Grok Build"]);
    for (const name of ["Pi", "Kiro", "Grok Build"]) expect(container.querySelector(`[aria-label="${name}"]`)?.hasAttribute("aria-disabled")).toBe(false);
    const missing = container.querySelector('[aria-label="Codex"]')!;
    expect(missing.hasAttribute("aria-disabled")).toBe(false);
    expect(missing.querySelector("header small")?.textContent).toBe("未安装");
    expect(missing.querySelector("button, summary, input, [tabindex] ")).toBeNull();
  });
  it("does not disable pending probes and updates ordering after an agent is installed and rescanned", async () => {
    let release!: (value: { installations: InstallationSummary[] }) => void;
    let installed = false;
    const pending = new Promise<{ installations: InstallationSummary[] }>(resolve => { release = resolve; });
    await mount({ scanAgent: async (id: string) => id === "kiro" ? installed
      ? { installations: [{ ...installation, providerId: id, status: "not_configured" }] } : pending
      : { installations: [] }, quotaSnapshots: vi.fn() } as unknown as CoreClient);
    const card = () => container.querySelector('[aria-label="Kiro"]')!;
    expect(card().hasAttribute("aria-disabled")).toBe(false);
    expect(card().textContent).toContain("检测中");
    await act(async () => release({ installations: [] }));
    expect(card().hasAttribute("aria-disabled")).toBe(false);
    installed = true;
    await act(async () => container.querySelector<HTMLButtonElement>(".welcome-rescan")!.click());
    expect(container.querySelector(".welcome-agent-card")?.getAttribute("aria-label")).toBe("Kiro");
    expect(card().hasAttribute("aria-disabled")).toBe(false);
    expect(card().textContent).toContain("未配置");
  });
  it("does not wait for unsupported quota queries on new native agents", async () => {
    const ids = ["pi", "opencode", "deepseek-harness"];
    const quotaSnapshots = vi.fn();
    await mount({ scanAgent: async (id: string) => ({ installations: ids.includes(id) ? [{ ...installation, providerId: id, installationId: id }] : [] }), quotaSnapshots } as unknown as CoreClient);
    expect(quotaSnapshots).not.toHaveBeenCalled();
    expect(container.querySelector('[aria-label="Pi"]')?.textContent).toContain("连接就绪");
    expect(container.querySelector('[aria-label="Pi"]')?.textContent).toContain("不支持查询剩余额度");
  });
  it("displays Cursor Models and Other Models as separate monthly allowances", async () => {
    const cursor = { ...installation, providerId: "cursor", installationId: "ins_cursor" };
    const q = { ...quota(), planType: "Pro · 重置 Oct 4（原生日期）", windows: [
      { id: "cursor-models", scope: "Cursor Models", windowMinutes: 0, usedPercent: 20, resetsAtUnix: null },
      { id: "cursor-other-models", scope: "Other Models", windowMinutes: 0, usedPercent: 100, resetsAtUnix: null },
    ] };
    await mount({ scanAgent: async (id: string) => ({ installations: id === "cursor" ? [cursor] : [] }),
      quotaSnapshots: async () => [{ providerId: "cursor", installationId: "ins_cursor", quota: q }],
    } as unknown as CoreClient);
    const card = container.querySelector('[aria-label="Cursor"]')!;
    for (const label of ["已登录", "Cursor Models", "Other Models", "80%", "0%", "每月", "10/4"]) expect(card.textContent).toContain(label);
    expect(card.querySelectorAll('[role="img"]')).toHaveLength(2);
    // Pool names ride inside each ring panel, right-aligned, so the card keeps
    // the same height as neighbours without headings above the rings.
    expect(card.querySelectorAll("h3")).toHaveLength(0);
    const panels = [...card.querySelectorAll(".welcome-window")];
    expect(panels.map(panel => panel.querySelector(".welcome-window-title em")?.textContent)).toEqual(["Cursor Models", "Other Models"]);
  });
  it("queries Kiro account allowance and displays its native billing date", async () => {
    const kiro = { ...installation, providerId: "kiro", installationId: "ins_kiro" };
    const resetDate = `${new Date().getFullYear()}-10-01`;
    const q = { ...quota(), planType: `KIRO FREE · 重置 ${resetDate}（原生日期）`, windows: [{ id: "kiro-included", scope: "套餐内（Kiro）", windowMinutes: 0, usedPercent: 0.16, resetsAtUnix: null }] };
    const quotaSnapshots = vi.fn(async () => [{ providerId: "kiro", installationId: "ins_kiro", quota: q }]);
    await mount({ scanAgent: async (id: string) => ({ installations: id === "kiro" ? [kiro] : [] }), quotaSnapshots } as unknown as CoreClient);
    expect(quotaSnapshots).toHaveBeenCalledWith(null, "ins_kiro");
    const card = container.querySelector('[aria-label="Kiro"]')!;
    for (const label of ["已登录", "100%", "重置时间：10/1"]) expect(card.textContent).toContain(label);
    expect(card.querySelector('[role="img"]')?.getAttribute("aria-label")).toContain("99.8%");
    expect(card.textContent).not.toContain("暂未接入");
    expect(card.textContent).not.toContain("KIRO FREE");
    expect(card.querySelector("details")?.textContent).toBe("安装位置/bin/codex");
  });
  it("rescans before querying installations and displays circular windows and reset dates", async () => {
    const scanned: string[] = [];
    const client = { scanAgent: vi.fn(async (id: string) => { scanned.push(id); return { installations: id === "codex" ? [installation] : [] } as BootstrapSnapshot; }), quotaSnapshots: vi.fn(async () => [view()]) } as unknown as CoreClient;
    await mount(client);
    expect(scanned).toEqual(supportedAgents.map(agent => agent.id));
    expect(container.querySelectorAll(".welcome-agent-card")).toHaveLength(12);
    expect(container.querySelectorAll(".welcome-ring-value")).toHaveLength(1);
    expect(container.textContent).toContain("75%"); expect(container.textContent).toContain("重置");
    expect(client.quotaSnapshots).toHaveBeenCalledWith(null, "ins_one");
    const retry = [...container.querySelectorAll("button")].find(b => b.textContent?.includes("重新检测"))!;
    await act(async () => retry.click());
    expect(client.scanAgent).toHaveBeenCalledTimes(24);
  });
  it("a failed discovery must not display uninstalled as a verified result", async () => {
    await mount({ scanAgent: async () => { throw new Error("service unavailable"); }, bootstrap: vi.fn(), quotaSnapshots: vi.fn() } as unknown as CoreClient);
    expect(container.querySelector("[role=alert]")).not.toBeNull();
    expect(container.textContent).not.toContain("未安装");
    expect(container.textContent).toContain("安装状态待确认");
  });
  it("does not let a slow account block the other account cards", async () => {
    let resolve!: (value: ReturnType<typeof view>[]) => void;
    const slow = new Promise<ReturnType<typeof view>[]>(r => { resolve = r; });
    const second = { ...installation, providerId: "kimi", installationId: "ins_two" };
    await mount({ scanAgent: async (id: string) => ({ installations: [installation, second].filter(i => i.providerId === id) }), quotaSnapshots: async (_: unknown, id: string) => id === "ins_two" ? slow : [view()] } as unknown as CoreClient);
    expect(container.textContent).toContain("已登录"); expect(container.textContent).toContain("验证授权中");
    await act(async () => resolve([]));
    expect(container.textContent).not.toContain("验证授权中");
  });
});


describe("independent native agent detection", () => {
  it("shows a fast card before another provider finishes discovery and starts all probes together", async () => {
    let release!: (s: BootstrapSnapshot) => void;
    const slow = new Promise<BootstrapSnapshot>(resolve => { release = resolve; });
    const scanAgent = vi.fn(async (id: string) => id === "kimi" ? slow : ({ installations: id === "codex" ? [installation] : [] } as unknown as BootstrapSnapshot));
    const quotaSnapshots = vi.fn(async () => [view()]);
    await mount({ scanAgent, quotaSnapshots } as unknown as CoreClient);
    expect(scanAgent).toHaveBeenCalledTimes(12);
    expect(container.querySelector('[aria-label="Codex"]')?.textContent).toContain("已登录");
    expect(container.querySelector('[aria-label="Codex"]')?.textContent).toContain("75%");
    expect(container.querySelector('[aria-label="Kimi Code"]')?.textContent).toContain("检测中");
    expect(container.querySelector('[aria-label="Cursor"]')?.textContent).toContain("未安装");
    await act(async () => release({ installations: [] } as unknown as BootstrapSnapshot));
    expect(container.querySelector('[aria-label="Kimi Code"]')?.textContent).toContain("未安装");
  });
  it("clears all previous readings and performs twelve fresh probes on each retry", async () => {
    let blocked = false;
    let release!: (s: BootstrapSnapshot) => void;
    const slow = new Promise<BootstrapSnapshot>(resolve => { release = resolve; });
    const scanAgent = vi.fn(async (id: string) => blocked ? slow : ({ installations: id === "codex" ? [installation] : [] } as unknown as BootstrapSnapshot));
    await mount({ scanAgent, quotaSnapshots: async () => [view()] } as unknown as CoreClient);
    expect(container.textContent).toContain("75%");
    blocked = true;
    await act(async () => [...container.querySelectorAll("button")].find(b => b.textContent?.includes("重新检测"))!.click());
    expect(scanAgent).toHaveBeenCalledTimes(24);
    expect(container.textContent).not.toContain("75%");
    expect(container.querySelectorAll(".welcome-status").length).toBe(12);
    await act(async () => release({ installations: [] } as unknown as BootstrapSnapshot));
  });
  it("isolates a provider failure from successful cards", async () => {
    await mount({ scanAgent: async (id: string) => { if (id === "kimi") throw new Error("timeout"); return { installations: id === "codex" ? [installation] : [] }; }, quotaSnapshots: async () => [view()] } as unknown as CoreClient);
    expect(container.querySelector('[aria-label="Codex"]')?.textContent).toContain("已登录");
    expect(container.querySelector('[aria-label="Kimi Code"]')?.textContent).toContain("检测失败");
    expect(container.querySelector('[aria-label="Kimi Code"]')?.textContent).not.toContain("未安装");
  });
});


describe("login observation and quota expiry", () => {
  it("keeps verified login after the clock expires quota without making another request", async () => {
    vi.useFakeTimers();
    const quotaSnapshots = vi.fn<CoreClient["quotaSnapshots"]>(async () => [view()]);
    await mount({ scanAgent: async (id: string) => ({ installations: id === "codex" ? [installation] : [] }), quotaSnapshots } as unknown as CoreClient);
    const card = () => container.querySelector('[aria-label="Codex"]')!.textContent!;
    expect(card()).toContain("已登录");
    await act(async () => { vi.advanceTimersByTime(90_000); });
    expect(card()).toContain("已登录");
    expect(card()).not.toContain("读数已过期，请重新检测");
    expect(card()).not.toContain("更新于");
    expect(card()).not.toContain("未取得有效账户读数");
    expect(quotaSnapshots).toHaveBeenCalledTimes(1);
    // A later explicit authentication failure must override the previous success.
    quotaSnapshots.mockReset();
    quotaSnapshots.mockImplementation(async () => [{ ...view(), quota: null, diagnostic: "quota_login_required" }]);
    await act(async () => [...container.querySelectorAll("button")].find(b => b.textContent?.includes("重新检测"))!.click());
    expect(card()).toContain("未登录");
    expect(card()).not.toContain("已登录");
  });
  it("does not verify a response that was already stale when received", () => {
    const q = quota();
    expect(welcomeStatus({ installation, view: view(q), observedAt: q.staleAfterUnixMs + 1 }).label).toBe("授权待确认");
  });
});

describe("detection lifecycle recovery", () => {
  it("restarts after effect cleanup and ignores responses from the previous client", async () => {
    let release!: (s: BootstrapSnapshot) => void;
    const old = new Promise<BootstrapSnapshot>(resolve => { release = resolve; });
    await mount({ scanAgent: async () => old, quotaSnapshots: async () => [view()] } as unknown as CoreClient);
    const fresh = { scanAgent: vi.fn(async () => ({ installations: [] })), quotaSnapshots: vi.fn() } as unknown as CoreClient;
    await act(async () => root!.render(<AgentWelcome client={fresh} onContinue={() => {}} renderIcon={() => null} />));
    expect(fresh.scanAgent).toHaveBeenCalledTimes(12);
    expect(container.textContent).not.toContain("正在检测…");
    await act(async () => release({ installations: [installation] } as unknown as BootstrapSnapshot));
    expect(container.querySelector('[aria-label="Codex"]')?.textContent).toContain("未安装");
    expect(fresh.quotaSnapshots).not.toHaveBeenCalled();
  });
  it("bounds an orphaned discovery reply and allows retry", async () => {
    vi.useFakeTimers();
    const scanAgent = vi.fn(async () => new Promise<BootstrapSnapshot>(() => {}));
    await mount({ scanAgent, quotaSnapshots: vi.fn() } as unknown as CoreClient);
    await act(async () => { vi.advanceTimersByTime(100_001); });
    expect(container.querySelectorAll(".welcome-status.warning")).toHaveLength(12);
    const button = [...container.querySelectorAll("button")].find(b => b.textContent?.includes("重新检测"))!;
    expect(button.disabled).toBe(false);
    scanAgent.mockImplementation(async () => ({ installations: [] } as unknown as BootstrapSnapshot));
    await act(async () => button.click());
    expect(container.querySelector('[aria-label="Cursor"]')?.textContent).toContain("未安装");
    expect(scanAgent).toHaveBeenCalledTimes(24);
  });
  it("bounds an orphaned quota reply without blocking other cards", async () => {
    vi.useFakeTimers();
    await mount({ scanAgent: async (id: string) => ({ installations: id === "codex" ? [installation] : [] }), quotaSnapshots: async () => new Promise(() => {}) } as unknown as CoreClient);
    expect(container.textContent).toContain("验证授权中");
    await act(async () => { vi.advanceTimersByTime(35_001); });
    expect(container.querySelector('[aria-label="Codex"]')?.textContent).toContain("检测失败");
    expect(container.textContent).not.toContain("验证授权中");
    expect([...container.querySelectorAll("button")].find(b => b.textContent?.includes("重新检测"))!.disabled).toBe(false);
  });
});


describe("welcome installation and upgrades", () => {
  it.each(["quota_exhausted", "quota_rate_limited"])("shows actionable %s status without a login entry", async diagnostic => {
    await mount({
      scanAgent: async (id: string) => ({ installations: id === "codex" ? [installation] : [] }),
      quotaSnapshots: async () => [{ ...view(), quota: null, diagnostic }],
    } as unknown as CoreClient);
    const card = container.querySelector('[aria-label="Codex"]')!;
    expect(card.querySelector('.welcome-status')?.textContent).toBe(diagnostic === "quota_exhausted" ? "额度已用尽" : "额度查询受限");
    expect(card.querySelector('.agent-login')).toBeNull();
    expect(card.textContent).not.toContain("授权待确认");
  });
  it("folds a rate-limited last reading into the header status line, not above the rings", async () => {
    const q = quota();
    await mount({
      scanAgent: async (id: string) => ({ installations: id === "codex" ? [installation] : [] }),
      quotaSnapshots: async () => [{ ...view(q), diagnostic: "quota_rate_limited" }],
    } as unknown as CoreClient);
    const card = container.querySelector('[aria-label="Codex"]')!;
    expect(card.querySelector(".welcome-outdated-note")).toBeNull();
    expect(card.querySelector("header small")?.textContent).toContain("已安装 · 上次读数 ");
    expect(card.querySelector(".welcome-card-quotas")?.classList.contains("outdated")).toBe(true);
    expect(card.querySelector(".welcome-status")?.textContent).toBe("额度查询受限");
  });
  it("opens the detected installation once, keeps login unverified, and removes the entry after a successful rescan", async () => {
    vi.mocked(isTauri).mockReturnValue(true);
    let release!: () => void;
    vi.mocked(invoke).mockImplementation(() => new Promise<void>(resolve => { release = resolve; }));
    let authenticated = false;
    await mount({ scanAgent: async (id: string) => ({ installations: id === "codex" ? [{ ...installation, status: authenticated ? "ready" : "not_configured", diagnostic: authenticated ? null : "authentication_failed" }] : [] }), quotaSnapshots: async () => [view()] } as unknown as CoreClient);
    const card = () => container.querySelector('[aria-label="Codex"]')!;
    const button = card().querySelector<HTMLButtonElement>('[aria-label="Codex：去登录"]')!;
    expect(button.closest(".welcome-status")?.textContent).toBe("未登录去登录");
    expect(card().textContent).not.toContain("codex login");
    await act(async () => { button.click(); button.click(); });
    expect(invoke).toHaveBeenCalledExactlyOnceWith("open_agent_login", { installationId: "ins_one" });
    expect(button.disabled).toBe(true);
    expect(card().textContent).toContain("打开中");
    await act(async () => release());
    expect(button.disabled).toBe(false);
    expect(button.textContent).toBe("去登录");
    expect(card().textContent).not.toContain("已打开登录入口");
    expect(card().textContent).toContain("未登录");
    authenticated = true;
    await act(async () => container.querySelector<HTMLButtonElement>(".welcome-rescan")!.click());
    expect(card().textContent).toContain("已登录");
    expect(card().querySelector(".agent-login")).toBeNull();
  });
  it("offers login for explicit auth failures but not custom providers, network failures, or demo data", async () => {
    await mount({
      scanAgent: async (id: string) => ({ installations: ["codex", "claude-code", "grok", "cursor", "kiro", "pi"].includes(id)
        ? [{ ...installation, providerId: id, installationId: `ins_${id}`, ...(id === "pi" ? { status: "not_configured", version: "demo" } : {}) }] : [] }),
      quotaSnapshots: async (_: unknown, id: string) => [{ installationId: id, providerId: id.slice(4), quota: null,
        diagnostic: id === "ins_codex" ? "quota_login_required" : id === "ins_claude-code" ? "claude_quota_login_required"
          : id === "ins_grok" ? "quota_network_error" : id === "ins_cursor" ? "claude_quota_custom_provider" : "quota_timeout" }],
    } as unknown as CoreClient);
    expect([...container.querySelectorAll(".agent-login-button")].map(button => button.getAttribute("aria-label")))
      .toEqual(["Codex：去登录", "Claude Code：去登录"]);
  });
  it("shows launch errors and lets the user retry", async () => {
    vi.mocked(isTauri).mockReturnValue(true);
    vi.mocked(invoke).mockRejectedValueOnce(new Error("launch failed")).mockResolvedValueOnce(undefined);
    await mount({ scanAgent: async (id: string) => ({ installations: id === "grok" ? [{ ...installation, providerId: id, status: "not_configured", diagnostic: "authentication_failed" }] : [] }) } as unknown as CoreClient);
    const button = container.querySelector<HTMLButtonElement>('[aria-label="Grok Build：去登录"]')!;
    await act(async () => button.click());
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("无法打开登录入口");
    expect(button.disabled).toBe(false);
    await act(async () => button.click());
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(button.textContent).toBe("去登录");
  });
  it("explains that browser previews cannot launch local clients", async () => {
    await mount({ scanAgent: async (id: string) => ({ installations: id === "grok" ? [{ ...installation, providerId: id, status: "not_configured", diagnostic: "authentication_failed" }] : [] }) } as unknown as CoreClient);
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Grok Build：去登录"]')!.click());
    expect(invoke).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("桌面端");
  });
  it("offers working official install links for all missing agents and opens the system browser", async () => {
    vi.mocked(isTauri).mockReturnValue(true);
    vi.mocked(invoke).mockResolvedValue(undefined);
    await mount({ scanAgent: async () => ({ installations: [] }) } as unknown as CoreClient);
    const links = [...container.querySelectorAll<HTMLAnchorElement>(".welcome-install-link")];
    expect(links).toHaveLength(12);
    for (const link of links) {
      expect(link.href).toMatch(/^https:\/\//);
      expect(link.closest('[aria-disabled="true"]')).toBeNull();
      await act(async () => link.click());
      expect(invoke).toHaveBeenLastCalledWith("open_website", { url: link.href });
    }
    vi.mocked(invoke).mockRejectedValueOnce(new Error("cannot open"));
    await act(async () => links[0].click());
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("无法打开浏览器");
  });
  it("keeps version and update controls out of the welcome page", async () => {
    const checkAgentUpdate = vi.fn();
    await mount({ scanAgent: async (id: string) => ({ installations: id === "codex" ? [installation] : [] }), quotaSnapshots: async () => [view()], checkAgentUpdate } as unknown as CoreClient);
    const card = container.querySelector('[aria-label="Codex"]')!;
    expect(card.querySelector("header small")?.textContent).toBe("已安装");
    expect(card.textContent).not.toMatch(/v1\.0|升级|检查更新/);
    expect(card.querySelector(".welcome-install-link")).toBeNull();
    expect(checkAgentUpdate).not.toHaveBeenCalled();
  });
});
