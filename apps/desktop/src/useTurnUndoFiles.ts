import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import type { ArtifactMetadata, TaskId } from "@dogcodes/protocol";
import { subscribeReviewChanges } from "./reviewWatch";
import { translate as t } from "./i18n";

type State = "ready" | "changed" | "restored" | "unchanged" | "incomplete" | "unverified" | "recovery" | "unavailable";
export function undoReason(state: State) {
  switch (state) {
    case "changed": return t("undo.changed");
    case "restored": case "unchanged": return t("undo.restored");
    case "unverified": return t("undo.unverified");
    case "incomplete": case "recovery": return t("undo.incomplete");
    default: return t("undo.statusFailed");
  }
}

export function useTurnUndoFiles(taskId: TaskId, metadata: ArtifactMetadata, paths: readonly string[] | undefined) {
  const ref = useRef<HTMLDetailsElement>(null);
  const key = JSON.stringify([taskId, metadata.artifactId, metadata.payloadSha256, paths]);
  const [result, setResult] = useState<{ key: string; states: Record<string, State> }>();
  const refresh = useRef<() => void>(() => {});
  useEffect(() => {
    if (!paths?.length) return;
    let visible = false, disposed = false, busy = false, dirty = false;
    let stop: (() => void) | undefined;
    const read = async () => {
      if (disposed || !visible) return;
      if (busy) { dirty = true; return; }
      busy = true;
      try {
        const states = await invoke<Record<string, State>>("core_turn_undo_files", {
          request: { taskId, artifactId: metadata.artifactId, expectedPayloadSha256: metadata.payloadSha256, paths },
        });
        if (!disposed) setResult({ key, states: states || {} });
      } catch { if (!disposed) setResult({ key, states: {} }); }
      finally { busy = false; if (dirty) { dirty = false; void read(); } }
    };
    const show = (next: boolean) => {
      if (visible === next) return;
      visible = next;
      stop?.();
      if (next) { void read(); stop = subscribeReviewChanges(taskId, read); }
    };
    refresh.current = read;
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(entries => show(entries.some(e => e.isIntersecting)));
    if (observer && ref.current) observer.observe(ref.current); else show(true);
    window.addEventListener("focus", read);
    return () => { window.removeEventListener("focus", read); disposed = true; stop?.(); observer?.disconnect(); };
  }, [key]);
  return { ref, states: result?.key === key ? result.states : undefined, refresh: () => refresh.current() };
}
