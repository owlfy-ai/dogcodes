// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ExecutorRef, QuotaSnapshot } from "@dogcodes/protocol";
import { QuotaSwitchNotice, quotaLooksExhausted, quotaResumesAt, quotaSwitchTarget } from "./QuotaSwitchNotice";

const NOW = Date.UTC(2026, 10, 7, 12);
const now = Math.floor(NOW / 1000);
const quota = (...windows: [used: number | null, resetsIn: number | null, scope?: string][]) => ({
  windows: windows.map(([usedPercent, resetsIn, scope], index) => ({ id: `w${index}`, windowMinutes: 300, usedPercent, resetsAtUnix: resetsIn == null ? null : now + resetsIn, scope })),
  collectedAtUnixMs: NOW, staleAfterUnixMs: NOW + 60_000, confidence: "provider_reported", source: "test", parserVersion: "1",
}) as unknown as QuotaSnapshot;
const executor = (providerId: string): ExecutorRef => ({ providerId, installationId: `ins_${providerId}` });
const codex = executor("codex"), claude = executor("claude-code"), kimi = executor("kimi");
let root: Root, host: HTMLDivElement;
const onSwitch = vi.fn();
const show = (props: Partial<Parameters<typeof QuotaSwitchNotice>[0]>) => act(async () => root.render(
  <form className="composer"><QuotaSwitchNotice current={codex} quota={null} turnExhausted={false} options={[]} running={false} lang="zh-CN" onSwitch={onSwitch} {...props} /><textarea /></form>));

beforeEach(() => {
  vi.useFakeTimers({ now: NOW });
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  onSwitch.mockClear();
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });

it("offers the first usable agent in the person's order, never the current or an exhausted one", () => {
  const options = [
    { executor: codex, quota: null },
    { executor: claude, quota: quota([100, 3600]) },
    { executor: kimi, quota: quota([20, 3600]) },
    { executor: executor("cursor"), quota: null },
  ];
  expect(quotaSwitchTarget(codex, options)).toEqual(kimi);
  expect(quotaSwitchTarget(codex, options.slice(0, 2))).toBeUndefined();
});

it("counts a second install of the same agent as somewhere else to go", () => {
  const other = { providerId: "codex", installationId: "ins_other" };
  expect(quotaSwitchTarget(codex, [{ executor: codex, quota: null }, { executor: other, quota: null }])).toEqual(other);
});

it("waits for the last used-up window, ignoring ones with room or already reset", () => {
  expect(quotaResumesAt(quota([100, 3600], [100, 7200], [50, 9000]))).toBe(now + 7200);
  expect(quotaResumesAt(quota([100, -60], [100, 600]))).toBe(now + 600);
  expect(quotaResumesAt(quota([100, 600, "model"]))).toBeUndefined();
  expect(quotaResumesAt(quota([100, null]))).toBeUndefined();
  expect(quotaResumesAt(null)).toBeUndefined();
});

it("believes a failed turn only until the agent reports room", () => {
  expect(quotaLooksExhausted(null, true)).toBe(true);
  expect(quotaLooksExhausted(quota([40, 3600]), true)).toBe(false);
  expect(quotaLooksExhausted(quota([100, 3600]), false)).toBe(true);
  expect(quotaLooksExhausted(quota([40, 3600]), false)).toBe(false);
});

it("stays out of the way unless the current agent is out of quota and idle", async () => {
  await show({ quota: quota([40, 3600]), options: [{ executor: claude, quota: null }] });
  expect(host.querySelector(".quota-switch-notice")).toBeNull();
  await show({ quota: quota([100, 3600]), options: [{ executor: claude, quota: null }], running: true });
  expect(host.querySelector(".quota-switch-notice")).toBeNull();
});

it("says when it comes back, switches on click and returns to the prompt", async () => {
  await show({ quota: quota([100, 3600]), options: [{ executor: codex, quota: null }, { executor: claude, quota: null }] });
  expect(host.querySelector('[role="status"]')?.textContent).toMatch(/^Codex 额度已用尽，预计 .+ 恢复。$/);
  const button = host.querySelector("button")!;
  expect(button.textContent).toBe("换 Claude Code 继续");
  expect(button.dataset.tooltip).toContain("对话内容");
  await act(async () => button.click());
  expect(onSwitch).toHaveBeenCalledWith(claude);
  expect(document.activeElement).toBe(host.querySelector("textarea"));
});

it("says so plainly when there's nowhere to switch", async () => {
  await show({ turnExhausted: true, options: [{ executor: codex, quota: null }] });
  expect(host.querySelector('[role="status"]')?.textContent).toBe("Codex 额度已用尽。 没有其他可用的 Agent。");
  expect(host.querySelector("button")).toBeNull();
});
