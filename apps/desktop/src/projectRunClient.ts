import { createInvalidationSubscriber } from "./invalidationWatch";
import { runOperations } from "./projectRunOperations";
import { availableAgentExecutors } from "./availableAgentExecutors";
import { invoke } from "@tauri-apps/api/core";
import type { ExecutorRef } from "@dogcodes/protocol";
import { terminalClient, followTerminal, terminalEnded, type TerminalApi, type TerminalInfo } from "./terminalClient";

export interface ProjectRunCommand { id: string; label: string; command: string; cwd: string; source: string; pinned?: boolean }
export interface ProjectRunDirectory { path: string; status: "analyzing" | "ready" | "error"; commands: ProjectRunCommand[]; error: string | null; userModified?: boolean }
export type ProjectRunEdit = { action: "rename"; label: string } | { action: "delete" } | { action: "pin"; pinned: boolean };
export interface ProjectRunCatalog { projectId: string; platform: string; shell: string; generation: string; executor: ExecutorRef | null; directories: ProjectRunDirectory[] }
export interface RunProcess { id: string; api: TerminalApi; stop: () => Promise<void> }
function process(id: string, onExit: () => void, start?: (cols: number, rows: number) => Promise<TerminalInfo>): RunProcess {
  let stopping = false, attempted = false;
  let dimensions: { cols: number; rows: number } | undefined;
  return {
    id,
    api: {
      ...terminalClient,
      open: async (_projectId, _index, cols, rows, receive) => {
        if (stopping && !attempted && start) { onExit(); throw new Error("运行已取消"); }
        dimensions ??= { cols, rows };
        const reconnectOnly = attempted || !start;
        attempted = true;
        let info: TerminalInfo | undefined;
        try { info = await terminalClient.get(id); } catch (error) { if (reconnectOnly) throw error; }
        if (!info && stopping) { onExit(); throw new Error("运行已取消"); }
        if (!info) info = await start!(dimensions.cols, dimensions.rows);
        if (stopping) await terminalClient.close(id);
        return followTerminal(info, event => { receive(event); if (event.kind === "state" && terminalEnded(event.info)) onExit(); });
      },
      attach: async (_id, receive) => followTerminal(await terminalClient.get(id), event => {
        receive(event); if (event.kind === "state" && terminalEnded(event.info)) onExit();
      }),
    },
    stop: async () => {
      stopping = true;
      if (attempted || !start) await terminalClient.close(id);
      else onExit();
    },
  };
}
const subscribe = createInvalidationSubscriber("core_project_runs_watch", "core_project_runs_unwatch");
export const projectRunClient = {
  watch: (taskId: string, refresh: () => void, error?: (message: string) => void) => subscribe(taskId, { taskId }, refresh, error),
  ...runOperations,
  get: (taskId: string) => invoke<ProjectRunCatalog>("core_project_runs", { taskId }),
  analyze: (taskId: string, executor: ExecutorRef | null, overwriteGeneration: string | undefined, expectedGeneration: string) => runOperations.mutate("core_project_runs_analyze", { operationId: crypto.randomUUID(), taskId, executor, overwriteGeneration: overwriteGeneration ?? null, expectedGeneration }),
  edit: (taskId: string, generation: string, directoryIndex: number, commandId: string, edit: ProjectRunEdit) => runOperations.mutate("core_project_run_edit", { operationId: crypto.randomUUID(), taskId, generation, directoryIndex, commandId, edit }),
  executors: availableAgentExecutors,
  list: terminalClient.list,
  restore: (info: TerminalInfo, onExit: () => void) => process(info.id, onExit),
  launch(taskId: string, catalog: ProjectRunCatalog, directoryIndex: number, commandId: string, onExit: () => void): RunProcess {
    const operationId = crypto.randomUUID();
    return process(operationId, onExit, (cols, rows) => invoke<TerminalInfo>("core_project_run_open", {
      taskId, generation: catalog.generation, directoryIndex, commandId, cols, rows, operationId,
    }));
  },
};
export type ProjectRunApi = typeof projectRunClient;
