import type {
  BootstrapSnapshot,
  ContextPreviewRequest,
  ContextStrength,
  ExecutorRef,
  TaskSummary,
} from "@dogcodes/protocol";

export function contextPreviewRequest(
  snapshot: BootstrapSnapshot,
  task: TaskSummary,
  targetExecutor: ExecutorRef,
  strength: ContextStrength,
): ContextPreviewRequest {
  const resumableTarget = snapshot.sessionMaps.find((mapping) =>
    mapping.taskId === task.taskId
    && mapping.executor.providerId === targetExecutor.providerId
    && mapping.executor.installationId === targetExecutor.installationId
    && (mapping.status === "resumable" || mapping.status === "active")
    && mapping.resumeCapability === "supported"
  );
  return {
    taskId: task.taskId,
    kind: resumableTarget ? "reconciliation" : "handoff",
    strength,
    sourceExecutor: task.preferredExecutor,
    targetExecutor,
    afterSequence: resumableTarget?.lastSeenSequence ?? 0,
    throughSequence: snapshot.lastSequence,
  };
}
