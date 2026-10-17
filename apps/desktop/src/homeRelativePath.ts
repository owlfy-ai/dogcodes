import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { homeDir } from "@tauri-apps/api/path";

let homeDirectory: Promise<string | null> | undefined;
function loadHomeDirectory(): Promise<string | null> {
  if (!isTauri()) return Promise.resolve(null);
  // Use the actual OS home directory, shared across all project surfaces.
  return homeDirectory ??= homeDir().catch(() => {
    homeDirectory = undefined;
    return null;
  });
}

export function useHomeDirectory(): string | null {
  const [home, setHome] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    void loadHomeDirectory().then(path => { if (!disposed) setHome(path); });
    return () => { disposed = true; };
  }, []);
  return home;
}

/** Abbreviate POSIX paths on a directory boundary; Windows paths stay intact. */
export function homeRelativePath(path: string, home: string | null): string {
  if (!home?.startsWith("/") || !path.startsWith("/")) return path;
  const base = home.replace(/\/+$/, "") || "/";
  if (path === base) return "~";
  const prefix = base === "/" ? "/" : `${base}/`;
  return path.startsWith(prefix) ? `~/${path.slice(prefix.length)}` : path;
}
