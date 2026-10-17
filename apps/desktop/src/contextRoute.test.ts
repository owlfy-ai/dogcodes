import { describe, expect, it } from "vitest";
import type { BootstrapSnapshot, TaskSummary } from "@dogcodes/protocol";
import { contextPreviewRequest } from "./contextRoute";

const task: TaskSummary = {
  taskId: "tsk_01K00000000000000000000000",
  projectId: "prj_01K00000000000000000000000",
  title: "Route context",
  preferredExecutor: { providerId: "fake-b", installationId: null },
  activePrimaryRun: null,
  streamVersion: 12,
};

function snapshot(): BootstrapSnapshot {
  return {
    projects: [],
    tasks: [task],
    installations: [],
    activeRuns: [],
    sessionMaps: [],
    lastSequence: 42,
    companion: {
      schemaVersion: 1,
      revision: 42,
      generatedAt: "2026-09-02T00:00:00Z",
      stale: false,
      workspaces: [],
      recentCompletion: null,
    },
  };
}

describe("contextPreviewRequest", () => {
  it("uses a full handoff when the target has no resumable native session", () => {
    const request = contextPreviewRequest(
      snapshot(),
      task,
      { providerId: "fake-c", installationId: null },
      "standard",
    );
    expect(request.kind).toBe("handoff");
    expect(request.afterSequence).toBe(0);
  });

  it.each(["resumable", "active"] as const)("uses the unseen increment for a %s target", (status) => {
    const current = snapshot();
    current.sessionMaps.push({model: null, thinking: null, 
      sessionMapId: "ses_01K00000000000000000000000",
      taskId: task.taskId,
      executor: { providerId: "fake-a", installationId: null },
      nativeSessionId: "fake_native_a",
      lastNativeTurnId: "native_turn_a",
      workspaceLocator: "/tmp/project",
      driverVersion: "0.1.0",
      nativeAgentVersion: null,
      createdAt: "2026-09-02T00:00:00Z",
      lastUsedAt: "2026-09-02T00:01:00Z",
      resumeCapability: "supported",
      status,
      lastSeenSequence: 17,
    });

    const request = contextPreviewRequest(
      current,
      task,
      { providerId: "fake-a", installationId: null },
      "compact",
    );
    expect(request.kind).toBe("reconciliation");
    expect(request.afterSequence).toBe(17);
    expect(request.throughSequence).toBe(42);
  });
});
