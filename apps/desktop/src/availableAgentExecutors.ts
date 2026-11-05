import { invoke } from "@tauri-apps/api/core";
import type { BootstrapSnapshot, ExecutorRef } from "@dogcodes/protocol";
import { installationsForTask, reviewAgentExecutors } from "./agentOptions";

/** Registered drivers can outlive an installation; intersect with current discovery. */
export async function availableAgentExecutors(taskId?: string): Promise<ExecutorRef[]> {
  const [snapshot, connected] = await Promise.all([
    invoke<BootstrapSnapshot>("core_bootstrap"),
    invoke<ExecutorRef[]>("core_model_ready_executors"),
  ]);
  return reviewAgentExecutors(taskId ? installationsForTask(snapshot, taskId) : snapshot.installations, connected);
}
