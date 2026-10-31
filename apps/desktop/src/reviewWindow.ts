import type { ReviewView } from "./ReviewPanel";
import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

export function useReviewWindow(taskId: string | null, onDock: () => void) {
  const [detached, setDetached] = useState(false);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState("");
  const callback = useRef(onDock); callback.current = onDock;
  const currentTask = useRef(taskId); currentTask.current = taskId;
  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    setDetached(false); setError("");
    if (!taskId || !isTauri()) return;
    void listen<string>("review-window-closed", event => {
      if (active && event.payload === taskId) { setDetached(false); callback.current(); }
    }).then(unlisten => { if (active) stop = unlisten; else unlisten(); });
    void invoke<boolean>("core_review_window_is_open", { taskId }).then(open => { if (active) setDetached(open); }).catch(e => { if (active) setError(userFacingError(e, t("reviewWindow.stateFailed"))); });
    return () => { active = false; stop?.(); };
  }, [taskId]);
  const detach = async (view?: ReviewView) => {
    if (!taskId || opening) return;
    setOpening(true); setError("");
    try {
      await invoke("core_review_window_open", { taskId, view });
      if (currentTask.current === taskId) setDetached(true);
    } catch (e) { if (currentTask.current === taskId) setError(userFacingError(e, t("reviewWindow.openFailed"))); }
    finally { setOpening(false); }
  };
  const dock = async () => {
    if (!taskId || !detached) return;
    try {
      await invoke("core_review_window_close", { taskId });
      if (currentTask.current === taskId) { setDetached(false); callback.current(); }
    } catch (e) { if (currentTask.current === taskId) setError(userFacingError(e, t("reviewWindow.dockFailed"))); }
  };
  return { detached, opening, error, detach, dock, available: isTauri() };
}
