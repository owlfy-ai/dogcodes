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
    expect(host.querySelector("button")?.textContent).toContain(ui.mode.edit);
    await click(host.querySelector("button")!);
    expect(host.textContent).toContain(ui.mode.read);
    expect(host.textContent).toContain(ui.mode.full);
    expect(host.textContent).toContain(ui.description.codexReadOnly);
    expect(host.textContent).toContain(ui.appliesNextTurn);
    if (lang !== "zh-CN" && lang !== "ja") expect(visibleText()).not.toMatch(/\p{Script=Han}/u);
    await click(host.querySelector("button")!);
  }
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
