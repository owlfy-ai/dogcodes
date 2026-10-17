import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CoreClient, ExecutorRef, TaskId, ProjectSummary } from "@dogcodes/protocol";

export const pickProjectDirectory = () => invoke<string | null>("core_pick_project_directory");

export async function openLocalProject(
  client: Pick<CoreClient, "registerProject" | "createTask">,
  executor: ExecutorRef,
): Promise<TaskId | null> {
  const path = await pickProjectDirectory();
  if (path === null) return null;
  // Registration validates and canonicalizes the path and reuses an existing
  // project in the native host, rather than trusting a stale UI snapshot.
  const projectId = await client.registerProject("", path);
  return client.createTask(projectId, "新对话", executor);
}

export async function revealLocalProject(project: ProjectSummary): Promise<void> {
  if (!isTauri()) throw new Error("请在桌面客户端中打开文件管理器。");
  await invoke("core_reveal_local_project", { projectId: project.projectId });
}
