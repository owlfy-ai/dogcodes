import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { translate as t } from "./i18n";

export const isWindowsPlatform = () => typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");

export function WindowsBrand({ logo }: { logo: string }) {
  return <div className="windows-brand">
    <img src={logo} alt="" aria-hidden="true" draggable={false} />
    <b>dog.codes</b>
  </div>;
}

/** Minimize / maximize / close for the frameless Windows main window. */
export function WindowsCaptionButtons() {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    const win = getCurrentWindow();
    let alive = true;
    const sync = () => { void win.isMaximized().then(value => { if (alive) setMaximized(value); }).catch(() => {}); };
    sync();
    const unlisten = win.onResized(sync);
    return () => { alive = false; void unlisten.then(dispose => dispose()).catch(() => {}); };
  }, []);
  const run = (action: "minimize" | "toggleMaximize" | "close") => () => { if (isTauri()) void getCurrentWindow()[action]().catch(() => {}); };
  const maximizeLabel = t(maximized ? "windowControls.restore" : "windowControls.maximize");
  return <div className="windows-caption" role="group">
    <button type="button" aria-label={t("windowControls.minimize")} data-tooltip={t("windowControls.minimize")} onClick={run("minimize")}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5.5h10" /></svg>
    </button>
    <button type="button" aria-label={maximizeLabel} data-tooltip={maximizeLabel} onClick={run("toggleMaximize")}>
      {maximized
        ? <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2.5 2.5v-2h7v7h-2M.5 2.5h7v7h-7z" /></svg>
        : <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M.5.5h9v9h-9z" /></svg>}
    </button>
    <button type="button" className="windows-caption-close" aria-label={t("windowControls.close")} data-tooltip={t("windowControls.close")} onClick={run("close")}>
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="m0 0 10 10M10 0 0 10" /></svg>
    </button>
  </div>;
}
