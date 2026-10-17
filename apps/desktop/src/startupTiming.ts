import { invoke, isTauri } from "@tauri-apps/api/core";
const sent = new Set<string>();
export function traceStartup(stage: string, elapsed = performance.now()) {
  if (!import.meta.env.DEV || !isTauri() || window.location.search) return;
  if (stage !== "event_loop_stall" && sent.has(stage)) return;
  sent.add(stage);
  void invoke("core_startup_trace", { stage: `frontend.${stage}`, elapsedMs: Math.round(elapsed) }).catch(() => {});
}
