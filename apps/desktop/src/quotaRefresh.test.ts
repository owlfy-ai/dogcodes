import { describe, expect, it, vi } from "vitest";
import type { CoreClient, QuotaSnapshotView } from "@dogcodes/protocol";
import { createQuotaRefresh } from "./quotaRefresh";

describe("independent quota refresh", () => {
  it("counts monetary provider balances as an available account reading", async () => {
    const view: QuotaSnapshotView = { installationId: "ins_pi", providerId: "pi", quota: {
      windows: [], balances: [{ label: "DeepSeek API", amount: 0, currency: "CNY" }],
      collectedAtUnixMs: 1000, staleAfterUnixMs: 2000, source: "provider_api", confidence: "provider_reported", parserVersion: "test",
    } };
    const client = { bootstrap: async () => ({ installations: [{ providerId: "pi", installationId: "ins_pi", status: "ready" }] }), quotaSnapshots: async () => [view] } as unknown as CoreClient;
    const update = vi.fn(), progress = vi.fn();
    const reader = createQuotaRefresh(client, null, update, progress);
    await reader.refreshNow();
    expect(update).toHaveBeenCalledWith(view);
    expect(progress.mock.lastCall?.[0]).toMatchObject({ unavailable: 0, completed: 1 });
    reader.dispose();
  });
  it("publishes fast providers without waiting for slow ones and deduplicates pending requests", async () => {
    let finish!: (views: QuotaSnapshotView[]) => void;
    const slow = new Promise<QuotaSnapshotView[]>(resolve => { finish = resolve; });
    const read = vi.fn((_task, id) => id === "slow" ? slow : Promise.resolve([{ installationId: id, providerId: id, quota: null }]));
    const client = { bootstrap: async () => ({ installations: [{ installationId: "slow" }, { installationId: "fast" }] }), quotaSnapshots: read } as unknown as CoreClient;
    const update = vi.fn();
    const reader = createQuotaRefresh(client, null, update);
    await reader.refresh();
    await Promise.resolve();
    expect(update.mock.calls.map(([v]) => v.installationId)).toEqual(["fast"]);
    await reader.refresh();
    expect(read.mock.calls.filter(([, id]) => id === "slow")).toHaveLength(1);
    reader.dispose();
    finish([{ installationId: "slow", providerId: "slow", quota: null }]);
    await Promise.resolve();
    expect(update.mock.calls.some(([v]) => v.installationId === "slow")).toBe(false);
  });

  it("picks up newly registered installations on notification refresh", async () => {
    const installations = [{ installationId: "first" }];
    const client = { bootstrap: async () => ({ installations }), quotaSnapshots: async (_task: unknown, id: string) => [{ installationId: id, providerId: id, quota: null }] } as unknown as CoreClient;
    const update = vi.fn();
    const reader = createQuotaRefresh(client, null, update);
    await reader.refresh();
    installations.push({ installationId: "new" });
    await reader.refresh();
    await Promise.resolve();
    expect(update.mock.calls.some(([v]) => v.installationId === "new")).toBe(true);
    reader.dispose();
  });
});

const observed = (id: string) => ({ installationId: id, providerId: id, quota: { windows: [{ id: "5h", windowMinutes: 300, usedPercent: 20 }] } }) as QuotaSnapshotView;

it("detects a newly logged-in agent before querying quota without blocking ready agents", async () => {
  let finishScan!: (snapshot: unknown) => void;
  const scanAgent = vi.fn(() => new Promise(resolve => { finishScan = resolve; }));
  const quotaSnapshots = vi.fn(async (_task, id) => [observed(id)]);
  const client = {
    bootstrap: async () => ({ installations: [
      { installationId: "grok-old", providerId: "grok", status: "not_configured" },
      { installationId: "codex", providerId: "codex", status: "ready" },
    ] }), scanAgent, quotaSnapshots,
  } as unknown as CoreClient;
  const update = vi.fn(), progress = vi.fn();
  const reader = createQuotaRefresh(client, null, update, progress);
  const completion = reader.refreshNow();
  expect(reader.refreshNow()).toBe(completion);
  await vi.waitFor(() => expect(update).toHaveBeenCalledWith(observed("codex")));
  expect(scanAgent).toHaveBeenCalledExactlyOnceWith("grok");
  expect(quotaSnapshots).not.toHaveBeenCalledWith(null, "grok-old", true);
  finishScan({ installations: [{ installationId: "grok-new", providerId: "grok", status: "ready" }] });
  await completion;
  expect(quotaSnapshots).toHaveBeenCalledWith(null, "grok-new", true);
  expect(update).toHaveBeenCalledWith(observed("grok-new"));
  expect(progress.mock.lastCall?.[0]).toMatchObject({ running: false, completed: 2, unavailable: 0 });
  reader.dispose();
});

it("does not rescan on automatic polling and still completes manual refresh if detection is refused", async () => {
  const scanAgent = vi.fn().mockRejectedValue(new Error("conversation running"));
  const quotaSnapshots = vi.fn(async () => []);
  const client = {
    bootstrap: async () => ({ installations: [{ installationId: "grok", providerId: "grok", status: "not_configured" }] }),
    scanAgent, quotaSnapshots,
  } as unknown as CoreClient;
  const progress = vi.fn();
  const reader = createQuotaRefresh(client, null, vi.fn(), progress);
  await reader.refresh();
  expect(scanAgent).not.toHaveBeenCalled();
  await reader.refreshNow();
  expect(scanAgent).toHaveBeenCalledExactlyOnceWith("grok");
  expect(quotaSnapshots).toHaveBeenLastCalledWith(null, "grok", true);
  expect(progress.mock.lastCall?.[0]).toMatchObject({ running: false, completed: 1, unavailable: 1, failed: false });
  reader.dispose();
});

it("does not dispatch quota or publish after disposal during login detection", async () => {
  let finishScan!: (snapshot: unknown) => void;
  const scanAgent = vi.fn(() => new Promise(resolve => { finishScan = resolve; }));
  const quotaSnapshots = vi.fn();
  const client = {
    bootstrap: async () => ({ installations: [{ installationId: "grok", providerId: "grok", status: "installed" }] }),
    scanAgent, quotaSnapshots,
  } as unknown as CoreClient;
  const progress = vi.fn();
  const reader = createQuotaRefresh(client, null, vi.fn(), progress);
  const completion = reader.refreshNow();
  await vi.waitFor(() => expect(scanAgent).toHaveBeenCalled());
  reader.dispose(); progress.mockClear();
  finishScan({ installations: [{ installationId: "grok", providerId: "grok", status: "ready" }] });
  await completion;
  expect(quotaSnapshots).not.toHaveBeenCalled();
  expect(progress).not.toHaveBeenCalled();
});

it("forces every installation, publishes fast results, and coalesces repeated manual clicks", async () => {
  let finish!: (views: QuotaSnapshotView[]) => void;
  const slow = new Promise<QuotaSnapshotView[]>(resolve => { finish = resolve; });
  const read = vi.fn((_task, id) => id === "slow" ? slow : Promise.resolve([observed(id)]));
  const client = { bootstrap: async () => ({ installations: [{ installationId: "slow" }, { installationId: "fast" }] }), quotaSnapshots: read } as unknown as CoreClient;
  const update = vi.fn(), progress = vi.fn();
  const reader = createQuotaRefresh(client, null, update, progress);
  const completion = reader.refreshNow();
  expect(reader.refreshNow()).toBe(completion);
  await vi.waitFor(() => expect(update).toHaveBeenCalledWith(observed("fast")));
  expect(progress).toHaveBeenLastCalledWith({ running: true, completed: 1, total: 2, unavailable: 0, failed: false });
  expect(read).toHaveBeenCalledWith(null, "slow", true);
  expect(read).toHaveBeenCalledWith(null, "fast", true);
  await reader.refresh();
  expect(read).toHaveBeenCalledTimes(2);
  finish([observed("slow")]);
  await completion;
  expect(progress).toHaveBeenLastCalledWith({ running: false, completed: 2, total: 2, unavailable: 0, failed: false });
  reader.dispose();
});

it("queues a fresh read after an in-flight automatic read instead of treating it as refreshed", async () => {
  let finish!: (views: QuotaSnapshotView[]) => void;
  const read = vi.fn().mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue([observed("one")]);
  const client = { bootstrap: async () => ({ installations: [{ installationId: "one" }] }), quotaSnapshots: read } as unknown as CoreClient;
  const reader = createQuotaRefresh(client, null, vi.fn());
  await reader.refresh();
  const completion = reader.refreshNow();
  await Promise.resolve();
  expect(read).toHaveBeenCalledTimes(1);
  finish([observed("one")]);
  await completion;
  expect(read).toHaveBeenLastCalledWith(null, "one", true);
  expect(read).toHaveBeenCalledTimes(2);
  reader.dispose();
});

it("counts unsupported and failed reads without discarding successful providers", async () => {
  const client = { bootstrap: async () => ({ installations: ["ok", "unknown", "offline"].map(installationId => ({ installationId })) }), quotaSnapshots: async (_task: unknown, id: string) => {
    if (id === "offline") throw new Error("private network details");
    return [id === "ok" ? observed(id) : { installationId: id, providerId: id, quota: null }];
  } } as unknown as CoreClient;
  const update = vi.fn(), progress = vi.fn();
  const reader = createQuotaRefresh(client, null, update, progress);
  await reader.refreshNow();
  expect(update).toHaveBeenCalledTimes(2);
  expect(progress).toHaveBeenLastCalledWith({ running: false, completed: 3, total: 3, unavailable: 2, failed: false });
  reader.dispose();
});

it("finishes a hung request, ignores its late response, and allows retry", async () => {
  vi.useFakeTimers();
  try {
    let finish!: (views: QuotaSnapshotView[]) => void;
    const read = vi.fn().mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue([observed("one")]);
    const client = { bootstrap: async () => ({ installations: [{ installationId: "one" }] }), quotaSnapshots: read } as unknown as CoreClient;
    const update = vi.fn(), progress = vi.fn();
    const reader = createQuotaRefresh(client, null, update, progress);
    const completion = reader.refreshNow();
    await vi.advanceTimersByTimeAsync(35_000);
    await completion;
    expect(progress.mock.lastCall?.[0]).toMatchObject({ running: false, unavailable: 1 });
    finish([observed("late")]);
    await Promise.resolve();
    expect(update).not.toHaveBeenCalled();
    await reader.refreshNow();
    expect(update).toHaveBeenCalledWith(observed("one"));
    reader.dispose();
  } finally { vi.useRealTimers(); }
});

it("reports discovery failure and ignores updates after disposal", async () => {
  const progress = vi.fn();
  const reader = createQuotaRefresh({ bootstrap: async () => { throw new Error("private"); } } as unknown as CoreClient, null, vi.fn(), progress);
  await reader.refreshNow();
  expect(progress.mock.lastCall?.[0]).toMatchObject({ running: false, failed: true });
  reader.dispose();
  progress.mockClear();
  await reader.refreshNow();
  expect(progress).not.toHaveBeenCalled();
});


it("keeps waiting for Antigravity quota without delaying other providers", async () => {
  vi.useFakeTimers();
  try {
    const update = vi.fn();
    const client = {
      bootstrap: async () => ({ installations: [{ installationId: "anti", providerId: "antigravity" }, { installationId: "fast", providerId: "codex" }] }),
      quotaSnapshots: async (_task: unknown, id: string) => id === "anti"
        ? new Promise(resolve => setTimeout(() => resolve([observed(id)]), 40_000)) : [observed(id)],
    } as unknown as CoreClient;
    const reader = createQuotaRefresh(client, null, update);
    const completion = reader.refreshNow();
    await vi.advanceTimersByTimeAsync(36_000);
    expect(update).toHaveBeenCalledWith(observed("fast"));
    expect(update).not.toHaveBeenCalledWith(observed("anti"));
    await vi.advanceTimersByTimeAsync(4_000);
    await completion;
    expect(update).toHaveBeenCalledWith(observed("anti"));
    reader.dispose();
  } finally { vi.useRealTimers(); }
});
