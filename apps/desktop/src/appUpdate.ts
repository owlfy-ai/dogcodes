import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** A newer dog.codes, already downloaded and verified by the desktop backend. */
export interface AppUpdate { version: string; currentVersion: string }

export interface AppUpdateApi {
  ready(): Promise<AppUpdate | null>;
  onReady(callback: (update: AppUpdate) => void): Promise<() => void>;
  /** Restarts into the new version; rejects with `busy`, `unavailable` or `failed`. */
  install(): Promise<void>;
}

export const tauriAppUpdateApi: AppUpdateApi = {
  ready: () => invoke<AppUpdate | null>("app_update_ready"),
  onReady: callback => listen<AppUpdate>("app-update-ready", event => callback(event.payload)),
  install: () => invoke("app_update_install"),
};

const LATER_KEY = "dogcodes.appUpdate.later";

/** "Later" holds for that version only: a newer release is offered again. */
export function laterVersion(): string | null {
  try { return localStorage.getItem(LATER_KEY); } catch { return null; }
}

export function rememberLater(version: string): void {
  try { localStorage.setItem(LATER_KEY, version); } catch { /* the notice still closes for this session */ }
}

export function shouldOfferUpdate(update: AppUpdate | null, later: string | null, running: boolean): update is AppUpdate {
  return Boolean(update) && update!.version !== later && !running;
}

export type AppUpdateFailure = "busy" | "failed";

export function installFailure(cause: unknown): AppUpdateFailure {
  return cause === "busy" ? "busy" : "failed";
}
