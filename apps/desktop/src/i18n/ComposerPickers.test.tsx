// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, ModelCatalog, PermissionCatalog, ProjectId, ProjectSummary } from "@dogcodes/protocol";
import { PermissionPicker } from "../PermissionPicker";
import { ModelPicker } from "../ModelPicker";
import { ProjectPicker } from "../ProjectPicker";
import { I18nProvider, LOCALES, type ResolvedLanguage } from "./index";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn(() => false) }));
let host: HTMLDivElement;
let root: Root;
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); localStorage.clear(); host = document.createElement("div"); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
const click = async (node: Element) => { await act(async () => node.dispatchEvent(new MouseEvent("click", { bubbles: true }))); };
const visibleText = () => [host.textContent ?? "", ...[...host.querySelectorAll("[aria-label], [data-tooltip], [placeholder]")]
  .map(node => ["aria-label", "data-tooltip", "placeholder"].map(name => node.getAttribute(name) ?? "").join(" "))].join(" ");
// The core reports Codex modes in Chinese; known ids must still follow the UI language.
const codexCatalog: PermissionCatalog = { defaultModeId: "default", modes: [
  { id: "read-only", label: "只读", description: "只读沙箱；需要扩大权限时请求审批。", dangerous: false },
  { id: "default", label: "默认", description: "工作区写入沙箱；受保护的操作请求审批。", dangerous: false },
  { id: "full-access", label: "完全访问", description: "关闭沙箱并跳过审批。", dangerous: true },
] };
const models: ModelCatalog = { defaultModel: "gpt", models: [
  { id: "gpt", label: "GPT-6.1-Sol", thinkingOptions: [{ id: "low", label: "low" }, { id: "xhigh", label: "xhigh" }, { id: "ultra", label: "ultra" }, { id: "turbo", label: "Turbo Mode" }], defaultThinking: "low" },
] };
const scratch: ProjectSummary = { projectId: "prj_scratch" as ProjectId, displayName: "快速对话", path: "/scratch", additionalDirectories: [], taskCount: 0 };

it("renders the permission picker in the UI language, including core-built mode descriptions", async () => {
  const client = { permissionCatalog: vi.fn(async () => codexCatalog) } as unknown as CoreClient;
  for (const lang of ["en", "ja", "zh-CN"] as ResolvedLanguage[]) {
    await act(async () => root.render(<I18nProvider language={lang}><PermissionPicker client={client} executor={{ providerId: "codex", installationId: "ins" }} taskId="tsk_a" disabled={false} onChange={() => {}} /></I18nProvider>));
    const ui = LOCALES[lang].permissionPicker;
    expect(host.querySelector("button")?.textContent).toContain(ui.mode.onRequest);
    await click(host.querySelector("button")!);
    expect(host.textContent).toContain(ui.mode.read);
    expect(host.textContent).toContain(ui.mode.full);
    expect(host.textContent).toContain(ui.description.codexReadOnly);
    expect(host.textContent).toContain(ui.appliesNextTurn);
    if (lang !== "zh-CN" && lang !== "ja") expect(visibleText()).not.toMatch(/\p{Script=Han}/u);
    await click(host.querySelector("button")!);
  }
});

const mode = (id: string, dangerous = false) => ({ id, label: "核心模式", description: "核心生成的说明", dangerous });
const agentCatalogs: Record<string, PermissionCatalog["modes"]> = {
  cursor: [mode("default"), mode("full-access", true)],
  codex: [mode("read-only"), mode("default"), mode("full-access", true)],
  "claude-code": ["plan", "default", "acceptEdits", "auto", "dontAsk"].map(id => mode(id)).concat(mode("bypassPermissions", true)),
  codebuddy: ["default", "acceptEdits", "plan", "auto", "dontAsk"].map(id => mode(id)).concat(mode("bypassPermissions", true), mode("fullAccess", true)),
  grok: [mode("ask"), mode("auto"), mode("always-approve", true)],
  zcode: ["build", "edit", "plan", "auto"].map(id => mode(id)).concat(mode("yolo", true)),
  kimi: [mode("default"), mode("yolo"), mode("plan"), mode("auto", true)],
  kiro: [mode("on", true), mode("off")],
  opencode: [mode("build"), mode("plan")],
  antigravity: [mode("desktop-approvals"), mode("plan"), mode("dangerously-skip-permissions", true)],
  pi: [mode("native", true)],
  "deepseek-harness": [mode("configured")],
};

it("translates every core-built permission mode and gives each mode its own icon", async () => {
  const iconByLabel = new Map<string, string>();
  const labelByIcon = new Map<string, string>();
  for (const [providerId, modes] of Object.entries(agentCatalogs)) {
    const client = { permissionCatalog: vi.fn(async () => ({ defaultModeId: modes[0].id, modes })) } as unknown as CoreClient;
    await act(async () => root.render(<I18nProvider language="en"><PermissionPicker client={client} executor={{ providerId, installationId: `ins_${providerId}` }} taskId="tsk_a" disabled={false} onChange={() => {}} /></I18nProvider>));
    await click(host.querySelector("button")!);
    const icons = [...host.querySelectorAll('[role="menuitemradio"] .permission-icon')].map(node => node.getAttribute("data-icon"));
    expect(icons, providerId).toHaveLength(modes.length);
    expect(new Set(icons).size, providerId).toBe(modes.length);
    expect(visibleText(), providerId).not.toMatch(/\p{Script=Han}/u);
    for (const item of host.querySelectorAll('[role="menuitemradio"]')) {
      const label = item.querySelector("strong")!.textContent!;
      const icon = item.querySelector(".permission-icon")!.getAttribute("data-icon")!;
      expect(item.className, `${providerId}: ${label}`).toBe("");
      expect(iconByLabel.get(label) ?? icon, `${providerId}: ${label}`).toBe(icon);
      expect(labelByIcon.get(icon) ?? label, `${providerId}: ${icon}`).toBe(label);
      iconByLabel.set(label, icon);
      labelByIcon.set(icon, label);
    }
    await act(async () => root.render(<></>));
  }
  expect(labelByIcon.size).toBe(10);
});

it("keeps native mode text and still distinguishes icons for unknown ACP agents", async () => {
  const modes = [
    { id: "default", label: "Default", description: "", dangerous: false },
    { id: "bypassPermissions", label: "Bypass", description: "Skip prompts", dangerous: true },
    { id: "fullAccess", label: "Everything", description: "No checks", dangerous: true },
  ];
  const client = { permissionCatalog: vi.fn(async () => ({ defaultModeId: "default", modes })) } as unknown as CoreClient;
  await act(async () => root.render(<I18nProvider language="en"><PermissionPicker client={client} executor={{ providerId: "new-agent", installationId: "ins_new" }} taskId="tsk_a" disabled={false} onChange={() => {}} /></I18nProvider>));
  await click(host.querySelector("button")!);
  const items = [...host.querySelectorAll('[role="menuitemradio"]')];
  expect(items.map(node => node.querySelector("strong")?.textContent)).toEqual(["Default", "Bypass", "Everything"]);
  expect(items[0].querySelector("small")).toBeNull();
  expect(items.map(node => node.querySelector(".permission-icon")?.getAttribute("data-icon"))).toEqual(["ask", "bypass", "full"]);
});

it("translates known reasoning efforts and keeps unknown agent labels", async () => {
  await act(async () => root.render(<I18nProvider language="en"><ModelPicker catalog={models} model={null} thinking={null} loading={false} error={null} disabled={false} onChange={() => {}} onRetry={() => {}} /></I18nProvider>));
  await click(host.querySelector('[aria-label="Select reasoning effort"]')!);
  const options = [...host.querySelectorAll(".model-options button")].map(node => node.textContent);
  expect(options).toEqual(["Low✓", "Extra high", "Ultra", "Turbo Mode"]);
  expect(host.querySelector('[role="dialog"]')?.getAttribute("aria-label")).toBe("Reasoning effort");
  expect(visibleText()).not.toMatch(/\p{Script=Han}/u);
});

it("shows the scratch project and project menu in the UI language", async () => {
  await act(async () => root.render(<I18nProvider language="en"><ProjectPicker projects={[scratch]} selectedProject={scratch} disabled={false} onSelect={() => {}} onOpenFolder={() => {}} /></I18nProvider>));
  expect(host.querySelector("button")?.textContent).toContain("No project");
  await click(host.querySelector("button")!);
  expect(host.querySelector("input")?.getAttribute("placeholder")).toBe("Search workspaces");
  expect(host.textContent).toContain("No projects connected yet");
  expect(host.textContent).toContain("Create project…");
  expect(host.textContent).toContain("Work without a project");
  expect(visibleText()).not.toMatch(/\p{Script=Han}/u);
});
