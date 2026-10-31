// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import type { CoreClient, InstallationSummary, QuotaSnapshotView } from "@dogcodes/protocol";
import { AgentWelcome } from "../AgentWelcome";
import { QuotaCenter } from "../App";
import { AgentLogin } from "../AgentLogin";
import { AgentUpdateControl, type AgentUpdateActions } from "../agentUpdates";
import { I18nProvider, LOCALES, type ResolvedLanguage } from "./index";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn(() => false), Channel: class {} }));
vi.mock("../WelcomeHerd", () => ({ WelcomeHerd: () => null }));
let root: Root | undefined;
let host: HTMLDivElement;
function setup() {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div"); document.body.append(host);
  root = createRoot(host);
}
afterEach(async () => {
  if (root) await act(async () => root!.unmount());
  root = undefined; host?.remove(); localStorage.clear(); vi.unstubAllGlobals();
});
const installation = (providerId: string): InstallationSummary => ({ providerId, installationId: `ins_${providerId}`,
  executable: `/bin/${providerId}`, fingerprint: "f", status: "ready", version: "1.2.3", versionText: null,
  diagnostic: null, probeElapsedMillis: 1, probedAt: "2026-10-30" });
const installations = [installation("deepseek-harness"), installation("opencode"), installation("codebuddy")];
const views = (): QuotaSnapshotView[] => installations.map(i => ({ providerId: i.providerId, installationId: i.installationId, diagnostic: null,
  quota: i.providerId === "codebuddy" ? null : { windows: i.providerId === "opencode" ? [
    { id: "opencode-go-rolling", scope: "OpenCode Go", windowMinutes: 300, usedPercent: 12.5 },
    { id: "opencode-go-weekly", scope: "OpenCode Go", windowMinutes: 10080, usedPercent: 20 },
    { id: "opencode-go-monthly", scope: "OpenCode Go", windowMinutes: 0, usedPercent: 30 },
  ] : [], balances: i.providerId === "deepseek-harness" ? [{ label: "DeepSeek API", amount: 12.5, currency: "CNY" }, { label: "DeepSeek API", amount: 0, currency: "USD" }] : [],
  collectedAtUnixMs: Date.now(), staleAfterUnixMs: Date.now() + 60_000, source: "provider_api", confidence: "provider_reported", parserVersion: "test" } }));
const languages = Object.keys(LOCALES) as ResolvedLanguage[];

it("switches every welcome card and its accessible text across all languages without rescanning", async () => {
  setup();
  const scanAgent = vi.fn(async () => ({ installations }));
  const quotaSnapshots = vi.fn(async () => views());
  const client = { scanAgent, quotaSnapshots } as unknown as CoreClient;
  for (const lang of languages) {
    await act(async () => root!.render(<I18nProvider language={lang}><AgentWelcome client={client} renderIcon={() => null} onContinue={() => {}} /></I18nProvider>));
    const dictionary = LOCALES[lang].agentUi;
    expect(host.textContent).toContain(dictionary.heroFirst);
    const balance = host.querySelector('[aria-label="DeepSeek Harness"]')!;
    expect(balance.textContent).toContain(dictionary.balanceVerified);
    expect(balance.querySelectorAll(".welcome-provider-balance")).toHaveLength(2);
    expect(balance.querySelector('[aria-label]')?.getAttribute("aria-label")).toBe(dictionary.providerBalance);
    expect(host.querySelector('[aria-label="OpenCode"]')?.textContent).toContain(dictionary.monthly);
    expect(host.querySelector('[aria-label="CodeBuddy"]')?.textContent).toContain(dictionary.quotaUnsupported);
    expect(host.textContent).not.toContain("agentUi.");
    if (["en", "fr", "de", "es", "pt-BR", "ru", "it", "ko"].includes(lang)) {
      expect(host.textContent).not.toMatch(/\p{Script=Han}/u);
      for (const item of host.querySelectorAll('[aria-label], [data-tooltip]')) expect(`${item.getAttribute("aria-label")} ${item.getAttribute("data-tooltip")}`).not.toMatch(/\p{Script=Han}/u);
    }
  }
  expect(scanAgent).toHaveBeenCalledTimes(12);
});

it("localizes management cards, missing agents, native update messages and tooltips", async () => {
  setup();
  const updateActions: AgentUpdateActions = { updates: { ins_opencode: { installationId: "ins_opencode", status: "managed", currentVersion: "1.2.3", latestVersion: "1.3.0", canUpgrade: false,
    source: "桌面应用 · 官方更新源", message: "此 CLI 随宿主应用更新，请在应用内检查更新", checkedAt: Date.now() } }, check: vi.fn(), upgrade: vi.fn(), openUpdater: vi.fn() };
  for (const lang of ["en", "ja", "fr", "de"] as ResolvedLanguage[]) {
    await act(async () => root!.render(<QuotaCenter views={views()} installations={installations} agentUpdates={updateActions}
      lang={lang} refreshProgress={{ running: false, total: 3, completed: 3, unavailable: 1, failed: false }} onRefresh={() => {}} onDetect={async () => {}} />));
    const dictionary = LOCALES[lang].agentUi;
    expect(host.textContent).toContain(dictionary.enabled);
    expect(host.textContent).toContain(dictionary.remainingQuota);
    expect(host.textContent).toContain(dictionary.providerBalance);
    expect(host.textContent).toContain(dictionary.notInstalled);
    expect(host.querySelector('[role="switch"]')?.getAttribute("aria-label")).toMatch(/DeepSeek Harness|OpenCode|CodeBuddy/);
    const meter = host.querySelector('[aria-label="OpenCode"] [role="progressbar"]')!;
    expect(meter.getAttribute("aria-valuenow")).toBe("87.5");
    expect(meter.querySelector("i")?.style.width).toBe("87.5%");
    if (lang === "de") expect(host.querySelector('[aria-label="OpenCode"]')?.textContent).toContain("87,5");
    if (lang !== "ja") {
      expect(host.textContent).not.toMatch(/\p{Script=Han}/u);
      for (const item of host.querySelectorAll('[aria-label], [data-tooltip]')) expect(`${item.getAttribute("aria-label")} ${item.getAttribute("data-tooltip")}`).not.toMatch(/\p{Script=Han}/u);
    }
  }
});

it("retranslates a login failure that remains visible when the language changes", async () => {
  setup();
  const render = (lang: ResolvedLanguage) => <I18nProvider language={lang}><AgentLogin installationId="ins_one" name="DeepSeek Harness" /></I18nProvider>;
  await act(async () => root!.render(render("en")));
  await act(async () => host.querySelector<HTMLButtonElement>("button")!.click());
  expect(host.querySelector('[role="alert"]')?.textContent).toBe(LOCALES.en.agentUi.loginDesktopOnly);
  await act(async () => root!.render(render("fr")));
  expect(host.querySelector('[role="alert"]')?.textContent).toBe(LOCALES.fr.agentUi.loginDesktopOnly);
});

it("localizes an in-flight upgrade without exposing a Chinese service message", async () => {
  setup();
  const actions: AgentUpdateActions = { updates: { ins_one: { installationId: "ins_one", status: "upgrading", currentVersion: "1", latestVersion: "2", source: "npm", message: "正在下载并安装，较大的更新可能需要几分钟", canUpgrade: false, checkedAt: 0 } }, check: vi.fn(), upgrade: vi.fn() };
  for (const lang of ["en", "fr"] as ResolvedLanguage[]) {
    await act(async () => root!.render(<I18nProvider language={lang}><AgentUpdateControl id="ins_one" actions={actions} /></I18nProvider>));
    expect(host.textContent).toContain(LOCALES[lang].agentUi.downloading);
    expect(host.textContent).not.toMatch(/\p{Script=Han}/u);
    expect(host.querySelector('[role="status"]')?.getAttribute("aria-label")).not.toMatch(/\p{Script=Han}/u);
  }
});
