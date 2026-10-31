import { watchDeletedOperationRecords } from "./deletedOperationRecords";
import { TauriCoreClient } from "./tauriClient";
import { traceStartup } from "./startupTiming";
import { lazy, Suspense, StrictMode, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { StartupShell } from "./StartupShell";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { AppErrorBoundary } from "./AppErrorBoundary";
import { StartupGate } from "./StartupGate";
import { SharedAgentPreferencesGate } from "./SharedAgentPreferencesGate";
import "./styles.css";
import { Tooltips } from "./Tooltips";
import { installFocusVisibility } from "./focusVisibility";

const disposeFocusVisibility = installFocusVisibility(document);
import.meta.hot?.dispose(disposeFocusVisibility);

const DetachedReview = lazy(() => import("./DetachedReview").then(module => ({ default: module.DetachedReview })));
const ComputerUseNotice = lazy(() => import("./ComputerUseNotice").then(module => ({ default: module.ComputerUseNotice })));

document.documentElement.classList.toggle("platform-macos", navigator.userAgent.includes("Macintosh"));

traceStartup("module_ready");
if (import.meta.env.DEV && !window.location.search) {
  let previous = performance.now();
  let foreground = document.visibilityState === "visible" && document.hasFocus();
  const timer = window.setInterval(() => {
    const now = performance.now();
    const active = document.visibilityState === "visible" && document.hasFocus();
    if (active && foreground && now - previous > 750) traceStartup("event_loop_stall", now - previous - 100);
    foreground = active;
    previous = now;
  }, 100);
  window.setTimeout(() => window.clearInterval(timer), 60000);
  import.meta.hot?.dispose(() => window.clearInterval(timer));
}

function RecoveryMaintenance() {
  useEffect(() => isTauri() ? watchDeletedOperationRecords(new TauriCoreClient()) : undefined, []);
  return null;
}

function RevealWindow() {
  useEffect(() => {
    if (!isTauri() || window.location.search) return;
    let second = 0;
    let revealed = false;
    const reveal = () => {
      if (revealed) return;
      revealed = true;
      void invoke("show_main_window").catch(() => {});
    };
    const first = requestAnimationFrame(() => { second = requestAnimationFrame(reveal); });
    // Some WebViews suspend animation frames while hidden. The committed DOM
    // and blocking stylesheet are ready here even when that happens.
    const fallback = window.setTimeout(reveal, 150);
    return () => { cancelAnimationFrame(first); cancelAnimationFrame(second); clearTimeout(fallback); };
  }, []);
  return null;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Tooltips />
    <RevealWindow />
    <RecoveryMaintenance />
    <AppErrorBoundary>
      {new URLSearchParams(window.location.search).has("computerNotice") ? <Suspense fallback={null}><ComputerUseNotice /></Suspense> : new URLSearchParams(window.location.search).get("reviewTask") ? <SharedAgentPreferencesGate><Suspense fallback={<StartupShell />}><DetachedReview taskId={new URLSearchParams(window.location.search).get("reviewTask")!} /></Suspense></SharedAgentPreferencesGate> : <StartupGate />}
    </AppErrorBoundary>
  </StrictMode>,
);
