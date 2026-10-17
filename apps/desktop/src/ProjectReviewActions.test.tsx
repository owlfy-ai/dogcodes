// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, ProjectReviewRequest, QuotaSnapshotView, TurnReviewRun } from "@dogcodes/protocol";
import { AgentQuotaContext } from "./AgentQuotaRing";
import { ProjectReviewActions } from "./ProjectReviewActions";
import type { ReviewSnapshot } from "./reviewClient";
let root: Root, container: HTMLDivElement;
const snapshot: ReviewSnapshot = { id: "snapshot-staged", taskId: "tsk_task", roots: [{ root: { id: "root", path: "/repo", git: true, scopes: [] }, mode: "staged", base: "base", target: "index", files: [{ path: "a.ts", kind: "added", before: null, after: null }], untracked: [], statistics: {}, error: null }] };
const result = (request: ProjectReviewRequest): TurnReviewRun => ({ workspaceReview: true, request, status: "completed", createdAt: "2026-09-22T10:00:00Z", finishedAt: null, nativeSessionId: null, sourceSequence: 1, sourceArtifact: null, inputSha256: "hash", coverage: "已暂存 · 固定快照", result: "发现边界问题", error: null });
const click = async (label: string) => act(async () => {
  const button = [...container.querySelectorAll("button")].find(b => b.textContent === label || b.getAttribute("aria-label") === label);
  expect(button).toBeDefined(); button!.click();
});
function fixture() {
  let runs: TurnReviewRun[] = [];
  const client = {
    bootstrap: vi.fn(async () => ({ installations: [{ providerId: "kimi", installationId: "k", status: "ready", version: "1.0" }] })),
    reviewExecutors: vi.fn(async () => [{ providerId: "kimi", installationId: "k" }]),
    quotaSnapshots: vi.fn(async () => []),
    turnReviews: vi.fn(async () => runs),
    startProjectReview: vi.fn(async (request: ProjectReviewRequest) => { const run = result(request); runs.push(run); return run; }),
    cancelTurnReview: vi.fn(async (_task: string, id: string) => { runs = runs.map(run => run.request.runId === id ? { ...run, status: "cancelled" } : run); }),
  };
  return { client, core: client as unknown as CoreClient, setRuns: (value: TurnReviewRun[]) => { runs = value; } };
}
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); localStorage.clear(); container = document.createElement("div"); document.body.append(container); root = createRoot(container); });
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
const render = (client: CoreClient, value: ReviewSnapshot | null = snapshot, disabled = false) => act(async () => root.render(<ProjectReviewActions taskId="tsk_task" snapshot={value} disabled={disabled} client={client}/>));
it("reviews the full comparison with the chosen Agent and uses the current snapshot after refresh", async () => {
  const f = fixture(); await render(f.core);
  for (const label of ["审查代码", "安全检查", "性能分析"]) {
    const button = container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
    expect(button.textContent).toBe("");
    expect(button.querySelector("svg")).not.toBeNull();
  }
  await click("选择项目审查 Agent");
  expect(container.querySelector(".agent-picker-menu .agent-option-quota")).not.toBeNull();
  expect(container.querySelector(".agent-picker-menu")?.textContent).not.toContain("关闭");
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
  expect(container.querySelector(".agent-picker-menu")).toBeNull();
  expect(document.activeElement?.getAttribute("aria-label")).toBe("选择项目审查 Agent");
  await click("选择项目审查 Agent");
  await act(async () => container.querySelector<HTMLButtonElement>(".agent-picker-menu button")!.click());
  await click("审查代码");
  expect(f.client.startProjectReview.mock.calls[0][0]).toMatchObject({ turnId: null, sourceSnapshot: "snapshot-staged", kind: "code", executor: { providerId: "kimi", installationId: "k" } });
  expect(container.textContent).not.toContain("发现边界问题");
  const execution = container.querySelector<HTMLDetailsElement>(".review-execution")!;
  expect(execution.open).toBe(false);
  await act(async () => { execution.open = true; execution.dispatchEvent(new Event("toggle", { bubbles: true })); });
  expect(execution.textContent).toContain("发现边界问题");
  await render(f.core, { ...snapshot, id: "snapshot-new" });
  expect(container.textContent).not.toContain("不代表当前 Diff 已通过审查");
  expect(container.textContent).not.toContain("已暂存 · 固定快照");
  expect(container.querySelector(".project-review-meta")).toBeNull();
  expect(container.textContent).not.toContain("重新检查");
  await click("审查代码");
  const [original, next] = f.client.startProjectReview.mock.calls.map(([request]) => request);
  expect(next.sourceSnapshot).toBe("snapshot-new");
  expect(next.retryOf).toBeNull();
  expect(next.runId).not.toBe(original.runId);
});
it("preserves the idempotency key after a lost response and disables empty or partial scopes", async () => {
  const f = fixture(); f.client.startProjectReview.mockRejectedValueOnce(new Error("connection lost"));
  await render(f.core); await click("安全检查"); await click("核实审查结果"); await click("重发原审查");
  expect(f.client.startProjectReview.mock.calls[0][0]).toEqual(f.client.startProjectReview.mock.calls[1][0]);
  for (const value of [null, { ...snapshot, roots: [] }, { ...snapshot, roots: snapshot.roots.map(r => ({ ...r, error: "failed" })) }]) {
    await render(f.core, value);
    for (const label of ["审查代码", "安全检查", "性能分析"]) {
      expect([...container.querySelectorAll("button")].find(b => b.getAttribute("aria-label") === label)?.disabled).toBe(true);
    }
  }
});
it("retries an interrupted review with its original scope and preserves the idempotency key on connection loss", async () => {
  const f = fixture();
  const original: ProjectReviewRequest = {taskId:'tsk_task', turnId:null, sourceSnapshot:'old-snapshot', runId:'failed-review', kind:'code', executor:{providerId:'kimi',installationId:'k'}, retryOf:null};
  f.setRuns([{...result(original), status:'failed', error:'Error: RetriableError: [internal] HTTP/2 keepalive ping timed out after 5000ms'}]);
  await render(f.core, {...snapshot, id:'new-snapshot'});
  expect(container.querySelector('.project-review-history-status')?.textContent).toBe('失败');
  expect(container.querySelector('[role="alert"]')?.textContent).toContain('审查尚未完成');
  f.client.startProjectReview.mockRejectedValueOnce(new Error('connection lost'));
  await click('重试审查');
  await click('核实审查结果'); await click('重发原审查');
  const [first, retry] = f.client.startProjectReview.mock.calls.map(([request])=>request);
  expect(first).toMatchObject({sourceSnapshot:'old-snapshot', retryOf:'failed-review', kind:'code', executor:original.executor});
  expect(first.runId).not.toBe(original.runId);
  expect(retry).toEqual(first);
});
it("restores project runs only and cancels a project review independently", async () => {
  const f = fixture();
  const request: ProjectReviewRequest = { taskId: "tsk_task", turnId: null, sourceSnapshot: snapshot.id, runId: "project", kind: "performance", executor: { providerId: "kimi", installationId: "k" }, retryOf: null };
  f.setRuns([{ ...result(request), status: "running" }, { ...result(request), request: { ...request, runId: "turn", turnId: "turn-1", sourceSnapshot: null }, result: "逐轮审查结果" }]);
  await render(f.core);
  expect(container.textContent).not.toContain("逐轮审查结果");
  await click("取消审查");
  expect(f.client.cancelTurnReview).toHaveBeenCalledWith("tsk_task", "project");
  expect(container.textContent).toContain("已取消");
});

it("shows shared quota readings on first opening and follows updates without querying providers", async () => {
  const f = fixture();
  const quota: NonNullable<QuotaSnapshotView["quota"]> = {
    windows: [{ id: "5h", windowMinutes: 300, usedPercent: 18 }],
    collectedAtUnixMs: Date.now(), staleAfterUnixMs: Date.now() + 60_000,
    source: "native_pull", confidence: "provider_reported", parserVersion: "test",
  };
  const views: QuotaSnapshotView[] = [
    { providerId: "kimi", installationId: "old", quota: { ...quota, windows: [{ id: "5h", windowMinutes: 300, usedPercent: 99 }] } },
    { providerId: "kimi", installationId: "k", quota },
  ];
  const show = (value: QuotaSnapshotView[]) => act(async () => root.render(
    <AgentQuotaContext.Provider value={value}>
      <ProjectReviewActions taskId="tsk_task" snapshot={snapshot} disabled={false} client={f.core}/>
    </AgentQuotaContext.Provider>,
  ));
  await show(views);
  await click("选择项目审查 Agent");
  const reading = () => container.querySelector(".agent-picker-menu .quota-ring-value")?.textContent;
  expect(reading()).toBe("82%");
  expect(f.client.quotaSnapshots).not.toHaveBeenCalled();

  await show([{ ...views[1], quota: { ...quota, windows: [{ id: "5h", windowMinutes: 300, usedPercent: 25 }] } }]);
  expect(reading()).toBe("75%");
  await click("选择项目审查 Agent");
  await click("选择项目审查 Agent");
  expect(reading()).toBe("75%");
  await show([]);
  expect(reading()).toBe("?");
  expect(f.client.quotaSnapshots).not.toHaveBeenCalled();
});

it("selects the newest report, preserves history state and does not switch when another review completes", async () => {
  vi.useFakeTimers();
  try {
    const f = fixture();
    const request = (runId: string, kind: ProjectReviewRequest["kind"]): ProjectReviewRequest => ({ taskId: "tsk_task", turnId: null, sourceSnapshot: snapshot.id, runId, kind, executor: { providerId: "kimi", installationId: "k" }, retryOf: null });
    const old = { ...result(request("old", "code")), createdAt: "2026-09-22T09:00:00Z", result: "旧审查详情" };
    const running = { ...result(request("running", "security")), createdAt: "2026-09-22T10:00:00Z", status: "running" as const };
    const latest = { ...result(request("latest", "performance")), createdAt: "2026-09-22T11:00:00Z" };
    f.setRuns([latest, old, running]);
    await render(f.core);
    const shown = () => container.querySelector<HTMLElement>('.project-review-results:not([hidden])')!;
    expect(shown().dataset.reviewId).toBe("latest");
    expect(container.querySelectorAll('.project-review-results:not([hidden])')).toHaveLength(1);
    expect(container.textContent).toContain("另有 1 项审查中");
    expect(container.querySelector('details.project-review-results')).toBeNull();
    async function choose(index: number) {
      await click("选择审查记录");
      const options = [...container.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')];
      expect(options.map(option => option.querySelector("b")?.textContent)).toEqual(["性能分析", "安全检查", "审查代码"]);
      await act(async () => options[index].click());
    }
    await choose(2);
    const oldPage = shown();
    const execution = oldPage.querySelector<HTMLDetailsElement>(".review-execution")!;
    await act(async () => { execution.open = true; execution.dispatchEvent(new Event("toggle", { bubbles: true })); oldPage.scrollTop = 120; oldPage.dispatchEvent(new Event("scroll")); });
    expect(execution.textContent).toContain("旧审查详情");
    await choose(0);
    await choose(2);
    expect(shown()).toBe(oldPage);
    expect(execution.open).toBe(true);
    expect(shown().scrollTop).toBe(120);
    f.setRuns([latest, old, { ...running, status: "completed" }]);
    await act(async () => vi.advanceTimersByTime(1500));
    expect(shown().dataset.reviewId).toBe("old");
    expect(container.textContent).not.toContain("另有 1 项审查中");
    await click("审查代码");
    expect(f.client.startProjectReview.mock.calls[0][0].retryOf).toBeNull();
    expect(shown().dataset.reviewId).toBe(f.client.startProjectReview.mock.calls[0][0].runId);
  } finally { vi.useRealTimers(); }
});

it("cancels only the selected review and supports dismissing the history menu", async () => {
  const f = fixture();
  const runs = ["code", "security"].map((kind, index) => ({
    ...result({ taskId: "tsk_task", turnId: null, sourceSnapshot: snapshot.id, runId: `active-${index}`, kind: kind as ProjectReviewRequest["kind"], executor: { providerId: "kimi", installationId: "k" }, retryOf: null }), status: "running" as const,
  }));
  f.setRuns(runs); await render(f.core);
  await click("选择审查记录");
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
  expect(container.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement?.getAttribute("aria-label")).toBe("选择审查记录");
  await click("选择审查记录");
  await act(async () => container.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')[1].click());
  await click("取消审查");
  expect(f.client.cancelTurnReview).toHaveBeenCalledWith("tsk_task", "active-0");
  expect(container.querySelector('.project-review-results:not([hidden])')?.getAttribute('data-review-id')).toBe("active-0");
});

it("collapses the review dock, remembers the choice and reopens when a review starts", async () => {
  const f = fixture();
  const expanded = vi.fn();
  const request: ProjectReviewRequest = { taskId: "tsk_task", turnId: null, sourceSnapshot: snapshot.id, runId: "done", kind: "code", executor: { providerId: "kimi", installationId: "k" }, retryOf: null };
  const show = () => act(async () => root.render(<ProjectReviewActions taskId="tsk_task" snapshot={snapshot} disabled={false} client={f.core} onExpandedChange={expanded}/>));
  await show();
  expect(container.querySelector(".project-review-toggle")?.tagName).toBe("SPAN");
  expect(container.textContent).toContain("对当前 1 个文件发起审查");
  expect(expanded).toHaveBeenLastCalledWith(false);
  f.setRuns([result(request)]);
  await act(async () => root.unmount()); root = createRoot(container); await show();
  const toggle = () => container.querySelector<HTMLButtonElement>("button.project-review-toggle")!;
  expect(toggle().getAttribute("aria-expanded")).toBe("true");
  expect(expanded).toHaveBeenLastCalledWith(true);
  await act(async () => toggle().click());
  expect(toggle().getAttribute("aria-expanded")).toBe("false");
  expect(container.querySelector<HTMLElement>(".project-review-body")?.hidden).toBe(true);
  expect(toggle().textContent).toContain("审查代码");
  expect(localStorage.getItem("dogcodes.review.collapsed")).toBe("1");
  expect(expanded).toHaveBeenLastCalledWith(false);
  await click("安全检查");
  expect(toggle().getAttribute("aria-expanded")).toBe("true");
  expect(localStorage.getItem("dogcodes.review.collapsed")).toBe("0");
});

it("recovers the original project review after reopening even when the current Diff changed", async () => {
  const f = fixture(); f.client.startProjectReview.mockRejectedValueOnce(new Error("offline"));
  await render(f.core); await click("审查代码");
  const original = f.client.startProjectReview.mock.calls[0][0];
  await act(async () => root.render(null)); await render(f.core, { ...snapshot, id: "new-diff" });
  expect(f.client.startProjectReview).toHaveBeenCalledTimes(1);
  expect(container.querySelector<HTMLButtonElement>('[aria-label="安全检查"]')!.disabled).toBe(true);
  await click("核实审查结果"); await click("重发原审查");
  expect(f.client.startProjectReview.mock.calls[1][0]).toEqual(original);
});
it("recovers an accepted project review from history when the response was lost", async () => {
  const f = fixture();
  f.client.startProjectReview.mockImplementationOnce(async request => { f.setRuns([result(request)]); throw new Error("reply lost"); });
  await render(f.core); await click("审查代码");
  expect(f.client.startProjectReview).toHaveBeenCalledTimes(1);
  expect(container.textContent).not.toContain("尚未核实");
  expect(container.querySelector('.project-review-history-status')?.textContent).toBe("已完成");
});
