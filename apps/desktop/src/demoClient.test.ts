import { describe, expect, it } from "vitest";
import { DemoCoreClient } from "./demoClient";

describe("DemoCoreClient", () => {
  it("does not report success when local Git capture is unavailable", async () => {
    const client = new DemoCoreClient();
    const initial = await client.bootstrap();
    const before = await client.timeline(initial.tasks[0].taskId);
    await expect(client.captureWorkspaceDiff(initial.tasks[0])).rejects.toThrow("浏览器 Demo");
    const after = await client.bootstrap();
    expect(after.tasks).toEqual(initial.tasks);
    expect(after.lastSequence).toBe(initial.lastSequence);
    expect(after.activeRuns).toEqual(initial.activeRuns);
    expect(await client.timeline(initial.tasks[0].taskId)).toEqual(before);
  });

  it("builds a versioned handoff preview without changing task state", async () => {
    const client = new DemoCoreClient();
    const initial = await client.bootstrap();
    const task = initial.tasks[0];

    const preview = await client.buildContextPreview({
      taskId: task.taskId,
      kind: "handoff",
      strength: "standard",
      sourceExecutor: task.preferredExecutor,
      targetExecutor: { providerId: "fake-peer", installationId: null },
      afterSequence: 0,
      throughSequence: initial.lastSequence,
    });

    expect(preview.package.schemaVersion).toBe(1);
    expect(preview.package.semanticSummary).toBeNull();
    expect(preview.package.provenance.sourceThroughSequence).toBe(initial.lastSequence);
    expect(preview.payloadSha256).toMatch(/^[a-f0-9]{64}$/);
    expect((await client.bootstrap()).tasks[0].preferredExecutor).toEqual(task.preferredExecutor);
  });

  it("confirms the reviewed hash before changing the preferred executor", async () => {
    const client = new DemoCoreClient();
    const initial = await client.bootstrap();
    const task = initial.tasks[0];
    const request = {
      taskId: task.taskId,
      kind: "handoff" as const,
      strength: "standard" as const,
      sourceExecutor: task.preferredExecutor,
      targetExecutor: { providerId: "fake-peer", installationId: null },
      afterSequence: 0,
      throughSequence: initial.lastSequence,
    };
    const preview = await client.buildContextPreview(request);

    await expect(client.confirmContextPackage(task, request, "0".repeat(64), null)).rejects.toThrow(
      "哈希已变化",
    );
    await client.confirmContextPackage(task, request, preview.payloadSha256, null);

    const confirmed = await client.bootstrap();
    expect(confirmed.tasks[0].preferredExecutor.providerId).toBe("fake-peer");
    expect((await client.timeline(task.taskId)).at(-1)?.kind).toBe("handoff");
    const page = await client.taskArtifacts(task.taskId);
    expect(page.items).toHaveLength(1);
    const metadata = page.items[0].metadata;
    expect(metadata.artifactId).toMatch(/^art_[0-9A-HJKMNP-TV-Z]{26}$/);
    const readRequest = { taskId: task.taskId, artifactId: metadata.artifactId,
      expectedPayloadSha256: metadata.payloadSha256, offset: 0, limit: 32768 };
    const chunk = await client.readArtifact(readRequest);
    expect(JSON.parse(new TextDecoder().decode(new Uint8Array(chunk.bytes)))).toEqual(preview.package);
    await expect(client.readArtifact({ ...readRequest, taskId: `tsk_${"0".repeat(26)}` })).rejects.toThrow("当前任务");
    await expect(client.readArtifact({ ...readRequest, expectedPayloadSha256: "0".repeat(64) })).rejects.toThrow("哈希");
    await expect(client.exportArtifact({ taskId: task.taskId, artifactId: metadata.artifactId,
      expectedPayloadSha256: metadata.payloadSha256, confirmedSensitiveExport: true })).rejects.toThrow("浏览器 Demo");
  });

  it("rehashes an edited draft before confirming the final version", async () => {
    const client = new DemoCoreClient();
    const initial = await client.bootstrap();
    const task = initial.tasks[0];
    const request = {
      taskId: task.taskId,
      kind: "handoff" as const,
      strength: "standard" as const,
      sourceExecutor: task.preferredExecutor,
      targetExecutor: { providerId: "fake-peer", installationId: null },
      afterSequence: 0,
      throughSequence: initial.lastSequence,
    };
    const base = await client.buildContextPreview(request);
    const editedPackage = structuredClone(base.package);
    editedPackage.pendingWork.push("Run final verification");
    const edited = await client.buildEditedContextPreview({
      request,
      basePayloadSha256: base.payloadSha256,
      editedPackage,
    });

    expect(edited.payloadSha256).not.toBe(base.payloadSha256);
    await client.confirmContextPackage(task, request, edited.payloadSha256, editedPackage);
    expect((await client.bootstrap()).tasks[0].preferredExecutor.providerId).toBe("fake-peer");
  });

  it("does not treat a handoff cancellation as a reconciliation opt-out", async () => {
    const client = new DemoCoreClient();
    const initial = await client.bootstrap();
    const task = initial.tasks[0];
    const request = {
      taskId: task.taskId,
      kind: "handoff" as const,
      strength: "standard" as const,
      sourceExecutor: task.preferredExecutor,
      targetExecutor: { providerId: "fake-peer", installationId: null },
      afterSequence: 0,
      throughSequence: initial.lastSequence,
    };
    const preview = await client.buildContextPreview(request);
    await expect(
      client.optOutReconciliation(task, request, preview.payloadSha256),
    ).rejects.toThrow("只能关闭 Reconciliation");
  });

  it("changes the preferred executor without mutating an active run", async () => {
    const client = new DemoCoreClient();
    const initial = await client.bootstrap();
    const task = initial.tasks[0];
    const activeProvider = task.preferredExecutor.providerId;
    const running = client.submitTurn(task, "test prompt");
    const duringRun = await client.bootstrap();
    expect(duringRun.tasks[0].activePrimaryRun?.executor.providerId).toBe(activeProvider);

    await client.setPreferredExecutor(duringRun.tasks[0], { providerId: "fake-peer", installationId: null });
    const switched = await client.bootstrap();
    expect(switched.tasks[0].activePrimaryRun?.executor.providerId).toBe(activeProvider);
    expect(switched.tasks[0].preferredExecutor.providerId).toBe("fake-peer");
    await running;
  });
});
