import { invoke } from "@tauri-apps/api/core";

export type TerminalStatus = "starting" | "running" | "stopping" | "exited" | "failed" | "unknown";
export interface TerminalInfo {
  id: string; epoch: string; projectId: string; directoryIndex: number;
  taskId: string | null; commandId: string | null; label: string; cwd: string; command: string | null;
  status: TerminalStatus; exitCode: number | null; error: string | null; outputEnd: number; createdAt: number;
  /** Current PTY size shared by every attached client; the latest resize wins. */
  cols?: number; rows?: number;
}
export interface OutputBatch { info: TerminalInfo; from: number; next: number; gap: boolean; bytes: number[] }
export type TerminalOutput = { kind: "data"; bytes: number[] } | { kind: "state"; info: TerminalInfo }
  | { kind: "gap" } | { kind: "disconnected"; error: string };
export interface TerminalConnection { id: string; detach: () => void }
export const terminalActive = (info: TerminalInfo) => ["starting", "running", "stopping"].includes(info.status);
export const terminalEnded = (info: TerminalInfo) => info.status === "exited" || info.status === "failed";

// One bounded long poll per attached view. Detach drops only this reader; the
// service drains the PTY regardless of whether any view is attached.
export function followTerminal(info: TerminalInfo, receive: (event: TerminalOutput) => void): TerminalConnection {
  let detached = false;
  receive({ kind: "state", info });
  void (async () => {
    let cursor = 0, state = "";
    try {
      while (!detached) {
        const batch = await invoke<OutputBatch>("core_terminal_read", { id: info.id, after: cursor });
        if (detached) return;
        if (batch.gap) receive({ kind: "gap" });
        if (batch.bytes.length) receive({ kind: "data", bytes: batch.bytes });
        cursor = batch.next;
        const current = JSON.stringify([batch.info.status, batch.info.exitCode, batch.info.error]);
        if (current !== state) { state = current; receive({ kind: "state", info: batch.info }); }
        // A completed process can still have more buffered output to drain.
        if (!terminalActive(batch.info) && cursor >= batch.info.outputEnd) return;
      }
    } catch (error) {
      if (!detached) receive({ kind: "disconnected", error: String(error) });
    }
  })();
  return { id: info.id, detach: () => { detached = true; } };
}
export const terminalClient = {
  open: async (projectId: string, directoryIndex: number, cols: number, rows: number,
    receive: (event: TerminalOutput) => void, operationId: string): Promise<TerminalConnection> => {
    let existing: TerminalInfo | undefined;
    try { existing = await invoke<TerminalInfo>("core_terminal_get", { id: operationId }); } catch { /* same-ID launch below */ }
    if (existing && (existing.projectId !== projectId || existing.directoryIndex !== directoryIndex || existing.taskId !== null)) throw new Error("终端操作标识已用于其他请求");
    const info = existing ?? await invoke<TerminalInfo>("core_terminal_open", { projectId, directoryIndex, cols, rows, operationId });
    return followTerminal(info, receive);
  },
  attach: async (id: string, receive: (event: TerminalOutput) => void): Promise<TerminalConnection> =>
    followTerminal(await invoke<TerminalInfo>("core_terminal_get", { id }), receive),
  list: (projectId: string) => invoke<TerminalInfo[]>("core_terminal_list", { projectId }),
  get: (id: string) => invoke<TerminalInfo>("core_terminal_get", { id }),
  write: (id: string, bytes: number[]) => invoke<void>("core_terminal_write", { id, bytes }),
  resize: (id: string, cols: number, rows: number) => invoke<void>("core_terminal_resize", { id, cols, rows }),
  close: (id: string) => invoke<void>("core_terminal_close", { id }),
};
export type TerminalApi = typeof terminalClient;
