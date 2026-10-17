import { Channel, invoke } from "@tauri-apps/api/core";
export type WorkspaceWatchMessage = { kind: "invalidated" | "heartbeat" } | { kind: "error"; code: string; message: string };
type Request = { taskId: string; root: number; path: string };
export function watchWorkspaceDirectory(request: Request, callbacks: { refresh: () => void; error: (message: string) => void; scopeChanged: () => void }): () => void {
  let stopped = false, attempt = 0, failures = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let id: string | undefined;
  const unwatch = (id: string) => { void invoke("core_workspace_unwatch", { id }).catch(() => {}); };
  const connect = () => {
    if (stopped) return;
    const current = ++attempt, subscription = crypto.randomUUID();
    id = subscription;
    let ended = false;
    const active = () => !stopped && current === attempt && !ended;
    const failed = (message: string) => {
      if (!active()) return;
      ended = true; unwatch(subscription);
      callbacks.error(`自动刷新已断开，正在重连：${message}`);
      callbacks.refresh();
      timer = setTimeout(connect, Math.min(15000, 1000 * 2 ** Math.min(failures++, 4)));
    };
    const changes = new Channel<WorkspaceWatchMessage>();
    changes.onmessage = event => {
      if (!active()) return;
      if (event.kind === "invalidated") callbacks.refresh();
      else if (event.kind === "error") {
        if (event.code === "scope_changed") callbacks.scopeChanged();
        failed(event.message);
      }
    };
    void invoke("core_workspace_watch", { ...request, id: subscription, changes }).then(() => {
      if (!active()) { unwatch(subscription); return; }
      failures = 0; callbacks.error("");
      // Watch first, then read a fresh snapshot, including after every reconnect.
      callbacks.refresh();
    }).catch(cause => failed(String(cause)));
  };
  connect();
  return () => { stopped = true; attempt++; clearTimeout(timer); if (id) unwatch(id); };
}
