import { createContext } from "react";
import type { CoreClient, ExecutorRef, TaskId, TaskSummary, PromptAttachment } from "@dogcodes/protocol";
import { contextPreviewRequest } from "./contextRoute";
import { translate as t } from "./i18n";

/** Main-Agent choices made in the composer that the core applies on the next submission. */
export const PendingExecutorContext = createContext<Record<string, ExecutorRef>>({});

const sameExecutor = (a: ExecutorRef, b: ExecutorRef) => a.providerId === b.providerId && a.installationId === b.installationId;

/** Hand the frozen history to `target` when it differs from the task's current executor. */
export async function switchExecutorWithAutomaticContext(client: CoreClient, taskId: TaskId, target: ExecutorRef): Promise<TaskSummary> {
  const latest = await client.bootstrap();
  let task = latest.tasks.find(item => item.taskId === taskId);
  if (!task) throw new Error(t("autoContext.unavailable"));
  if (task.activePrimaryRun) throw new Error(t("autoContext.waitReply"));
  if (!sameExecutor(task.preferredExecutor, target)) {
    const request = contextPreviewRequest(latest, task, target, "standard");
    const built = await client.buildContextPreview(request);
    await client.confirmContextPackage(task, request, built.payloadSha256, null);
    task = (await client.bootstrap()).tasks.find(item => item.taskId === taskId);
    if (!task) throw new Error(t("autoContext.unavailable"));
  }
  if (!sameExecutor(task.preferredExecutor, target)) throw new Error(t("autoContext.executorChanged"));
  return task;
}

/** Freeze history at send time, after any preceding run has finished. */
export async function submitWithAutomaticContext(
  client: CoreClient, taskId: TaskId, target: ExecutorRef, prompt: string, model: string | null, thinking: string | null = null, permissionMode: string | null = null, attachments: PromptAttachment[] = [],
): Promise<void> {
  const task = await switchExecutorWithAutomaticContext(client, taskId, target);
  await client.submitTurn(task, prompt, model, thinking, permissionMode, attachments);
}
