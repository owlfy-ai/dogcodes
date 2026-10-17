// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, TimelineItem, TurnReviewRequest, TurnReviewRun } from "@dogcodes/protocol";
import { AgentQuotaContext } from "./AgentQuotaRing";
import { ConversationPages } from "./ConversationPages";
import { TurnActions } from "./TurnActions";
import { frozenDiff, editPatch } from "./conversationDiff";
import type { QuotaSnapshotView } from "@dogcodes/protocol";

let root: Root, container: HTMLDivElement;
const date = "2026-09-19T10:00:00Z";
const item = (kind: string, text: string): TimelineItem => ({attachments: [],  itemId: kind, taskId: "tsk_source", turnId: "trn_old", kind, text, sequence: 2, occurredAt: date, providerId: "codex" });
const metadata = { artifactId: "art_old", payloadSha256: "a".repeat(64), byteLength: 10, createdAt: date };
const items = [item("assistant_message", "本轮最后的回复"), item("turn_files", JSON.stringify(metadata))];
const completed = (request: TurnReviewRun["request"]): TurnReviewRun => ({ workspaceReview: true, request, status: "completed", createdAt: date, finishedAt: date, nativeSessionId: "separate-session", sourceSequence: 42, sourceArtifact: null, inputSha256: "hash", coverage: "仅检查本轮历史材料", result: "发现一个边界问题", error: null });
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
  HTMLDialogElement.prototype.showModal = function () { this.setAttribute("open", ""); };
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
const textButton = (text: string) => [...container.querySelectorAll<HTMLButtonElement>("button")].find(node => node.textContent === text)!;
const button = (label: string) => container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
async function click(target: HTMLElement) { await act(async () => target.click()); }
async function mount(client: CoreClient, values = items) {
  await act(async () => root.render(<TurnActions client={client} items={values} turnId="trn_old" startedAt={date} completedAt={date}/>));
}
function fixture() {
  let runs: TurnReviewRun[] = [];
  const client = {
    bootstrap: vi.fn(async () => ({installations: [
      {providerId:"codex",installationId:"a",status:"ready",version:"1.0",executable:"/bin/codex"},
      {providerId:"claude-code",installationId:"b",status:"ready",version:"1.0",executable:"/bin/claude"},
    ]})),
    reviewExecutors: vi.fn(async () => [{ providerId: "codex", installationId: "a" }, { providerId: "claude-code", installationId: "b" }]),
    turnReviews: vi.fn(async () => runs),
    startTurnReview: vi.fn(async (request: TurnReviewRequest) => { const run = completed(request); runs = [...runs, run]; return run; }),
    cancelTurnReview: vi.fn(async (_task: string, id: string) => { runs = runs.map(run => run.request.runId === id ? { ...run, status: "cancelled" } : run); }),
    readArtifact: vi.fn(async () => { throw new Error("missing historical snapshot"); }),
    readWorkspaceFile: vi.fn(),
  };
  return { client, core: client as unknown as CoreClient, setRuns: (value: TurnReviewRun[]) => { runs = value; } };
}
it("uses the chosen reviewer and retries the original run without changing the main Agent", async () => {
  const f = fixture(); await mount(f.core);
  await click(button("选择审查 Agent"));
  const claude = [...container.querySelectorAll<HTMLButtonElement>(".agent-picker-menu button")].find(b => b.textContent?.includes("Claude"))!;
  await click(claude); await click(button("审查代码"));
  const request = f.client.startTurnReview.mock.calls[0][0];
  expect(request).toMatchObject({ taskId: "tsk_source", turnId: "trn_old", kind: "code", executor: { providerId: "claude-code" }, retryOf: null });
  expect(localStorage.getItem("dogcodes.review.executor")).toBe("claude-code:b");
  expect(container.textContent).not.toContain("发现一个边界问题");
  const execution = container.querySelector<HTMLDetailsElement>(".review-execution")!;
  expect(execution.open).toBe(false);
  await act(async () => { execution.open = true; execution.dispatchEvent(new Event("toggle", { bubbles: true })); });
  expect(execution.textContent).toContain("发现一个边界问题");
  await click(container.querySelector<HTMLButtonElement>(".turn-review-result footer button")!);
  const retry = f.client.startTurnReview.mock.calls[1][0];
  expect(retry.retryOf).toBe(request.runId); expect(retry.runId).not.toBe(request.runId);
});
it("cancels only the selected review and recovers persisted results", async () => {
  const f = fixture();
  f.setRuns([{ ...completed({ taskId: "tsk_source", turnId: "trn_old", runId: "run_saved", kind: "tests", executor: { providerId: "codex", installationId: "a" }, retryOf: null }), status: "running", result: "" }]);
  await mount(f.core); expect(container.textContent).toContain("检查中");
  await click(container.querySelector<HTMLButtonElement>(".turn-review-result footer button")!);
  expect(f.client.cancelTurnReview).toHaveBeenCalledWith("tsk_source", "run_saved");
  expect(container.textContent).toContain("已取消");
});
it("reuses the request id after an uncertain response", async () => {
  const f = fixture(); f.client.startTurnReview.mockRejectedValueOnce(new Error("connection lost"));
  await mount(f.core);
  await click(button("审查代码")); await click(textButton("核实审查结果")); await click(textButton("重发原审查"));
  expect(f.client.startTurnReview.mock.calls[0][0]).toEqual(f.client.startTurnReview.mock.calls[1][0]);
  expect(button("从此轮分叉")).toBeNull();
  expect(button("复制本轮回复")).not.toBeNull();
});
it("allows current-code checks without a saved snapshot", async () => {
  const f = fixture();
  await mount(f.core, [items[0]]);
  for (const label of ["审查代码", "安全检查", "性能分析"]) expect(button(label).disabled).toBe(false);
});
it("keeps distinct root paths when combining saved patches and marks binary snapshots unavailable", () => {
  const diff = frozenDiff({ roots: ["/root/one", "/root/two"].map(workspacePath => ({ workspacePath, changes: { patch: editPatch("a.ts", "before\n", "after\n") } })), files: [{ path: "/root/one/a.ts" }, { path: "/root/two/a.ts" }, { path: "binary", before: { hex: "00" }, after: { hex: "01" } }] });
  expect(diff.kind).toBe("structured");
  if (diff.kind === "structured") expect(diff.diff.files.map(f => f.newPath)).toEqual(["/root/one/a.ts", "/root/two/a.ts"]);
});

it("shows the backend-selected Agent and dispatches Kimi", async () => {
  const f = fixture();
  const entries = [
    {providerId:"codex",installationId:"new",status:"ready",version:"2.0",executable:"/bin/new"},
    {providerId:"kimi",installationId:"kimi",status:"ready",version:"1.0",executable:"/bin/kimi"},
  ];
  f.client.bootstrap.mockResolvedValue({installations: entries});
  f.client.reviewExecutors.mockResolvedValue(entries.map(i => ({providerId:i.providerId, installationId:i.installationId})));
  localStorage.setItem("dogcodes.review.executor", "codex:old");
  await mount(f.core); await click(button("审查代码"));
  expect(f.client.startTurnReview.mock.calls[0][0].executor.installationId).toBe("new");
  await click(button("选择审查 Agent"));
  const choices = [...container.querySelectorAll<HTMLButtonElement>(".agent-picker-menu button[aria-pressed]")];
  expect(choices.map(b => b.querySelector(".agent-option-label")?.textContent)).toEqual(["Codex", "Kimi"]);
  await click(choices[1]); await click(button("安全检查"));
  expect(f.client.startTurnReview.mock.calls[1][0].executor.providerId).toBe("kimi");
});

it("shares live quota readings with cached conversations and closes without a Close menu item", async () => {
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  const f = fixture();
  const views = (usedPercent: number): QuotaSnapshotView[] => [{ providerId:"codex",installationId:"a",diagnostic:null,quota:{windows:[{id:"five-hour",windowMinutes:300,usedPercent,resetsAtUnix:null,scope:null}],context:null,collectedAtUnixMs:Date.now(),staleAfterUnixMs:Date.now()+60_000,planType:null,source:"native",confidence:"provider_reported",parserVersion:"test"} }];
  const revision = {};
  const render = async (used: number) => act(async () => root.render(<AgentQuotaContext.Provider value={views(used)}><ConversationPages taskId="tsk_source" revision={revision}><TurnActions client={f.core} items={items} turnId="trn_old" startedAt={date} completedAt={date}/></ConversationPages></AgentQuotaContext.Provider>));
  await render(25); await click(button("选择审查 Agent"));
  expect(container.querySelector('.agent-picker-menu .agent-option-quota')?.textContent).toBe("75%");
  expect(container.querySelector('.agent-picker-menu')?.textContent).not.toContain("关闭");
  await render(40);
  expect(container.querySelector('.agent-picker-menu .agent-option-quota')?.textContent).toBe("60%");
  const rings = container.querySelectorAll('.agent-picker-menu .agent-option-quota');
  expect(rings[1].getAttribute('aria-label')).toContain("额度暂无读数");
  await act(async () => document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true})));
  expect(container.querySelector('.agent-picker-menu')).toBeNull();
  expect(document.activeElement).toBe(button("选择审查 Agent"));
  await click(button("选择审查 Agent"));
  await act(async () => document.body.dispatchEvent(new Event('pointerdown',{bubbles:true})));
  expect(container.querySelector('.agent-picker-menu')).toBeNull();
});

it("offers only the three supported checks and keeps retired results readable without retry", async () => {
  const f = fixture();
  f.setRuns((["tests", "requirements", "impact"] as const).map(kind => completed({ taskId: "tsk_source", turnId: "trn_old", runId: `run_${kind}`, kind, executor: { providerId: "codex", installationId: "a" }, retryOf: null })));
  await mount(f.core);
  for (const label of ["检查测试", "对照要求", "评估影响"]) expect(button(label)).toBeNull();
  expect(container.querySelector('[aria-label="更多检查"]')).toBeNull();
  expect(container.querySelectorAll(".turn-review-result")).toHaveLength(3);
  expect(container.querySelector(".turn-review-result footer button")).toBeNull();
  for (const label of ["审查代码", "安全检查", "性能分析"]) await click(button(label));
  expect(f.client.startTurnReview.mock.calls.map(([request]) => request.kind)).toEqual(["code", "security", "performance"]);
});

it("restores a pending turn review after remount without generating another run id", async () => {
  const f = fixture(); f.client.startTurnReview.mockRejectedValueOnce(new Error("offline"));
  await mount(f.core); await click(button("审查代码"));
  const original = f.client.startTurnReview.mock.calls[0][0];
  await act(async () => root.render(null)); await mount(f.core);
  expect(f.client.startTurnReview).toHaveBeenCalledTimes(1);
  expect(button("性能分析").disabled).toBe(true);
  await click(textButton("核实审查结果")); await click(textButton("重发原审查"));
  expect(f.client.startTurnReview.mock.calls[1][0]).toEqual(original);
});
