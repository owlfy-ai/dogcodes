import { useSyncExternalStore } from "react";

export const SKIP_STAGE_ALL_CONFIRMATION_KEY = "dogcodes.git.skip-stage-all-confirmation.v1";
const changedEvent = "dogcodes:git-commit-preferences";
let fallback = false;
let memoryOnly = false;

export function readSkipStageAllConfirmation(): boolean {
  if (memoryOnly) return fallback;
  try { return localStorage.getItem(SKIP_STAGE_ALL_CONFIRMATION_KEY) === "true"; }
  catch { return fallback; }
}

export function setSkipStageAllConfirmation(skip: boolean) {
  fallback = skip;
  try { localStorage.setItem(SKIP_STAGE_ALL_CONFIRMATION_KEY, String(skip)); memoryOnly = false; }
  catch { memoryOnly = true; }
  window.dispatchEvent(new Event(changedEvent));
}

function subscribe(listener: () => void) {
  const onStorage = (event: StorageEvent) => {
    if (event.key === SKIP_STAGE_ALL_CONFIRMATION_KEY || event.key === null) { memoryOnly = false; listener(); }
  };
  window.addEventListener(changedEvent, listener);
  window.addEventListener("storage", onStorage);
  return () => {
    window.removeEventListener(changedEvent, listener);
    window.removeEventListener("storage", onStorage);
  };
}

export const useSkipStageAllConfirmation = () => useSyncExternalStore(subscribe, readSkipStageAllConfirmation, () => false);
