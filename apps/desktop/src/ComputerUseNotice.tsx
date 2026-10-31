import { useEffect, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ComputerUseActivity } from "@dogcodes/protocol";
import { I18nProvider, UI_LANGUAGE_KEY, useI18n, type UiLanguage } from "./i18n";
import "./computerUseNotice.css";

const idle: ComputerUseActivity = { operationId:null, taskId:null, tool:null, mode:"idle", preparing:false, foregroundPossible:false };

function captionInset() {
  const value = Number(new URLSearchParams(window.location.search).get("captionTop"));
  return Number.isFinite(value) ? Math.min(200, Math.max(0, value)) : 0;
}

export function ComputerUseNotice() {
  const [activity, setActivity] = useState(idle);
  const [disconnected, setDisconnected] = useState(false);
  const [language, setLanguage] = useState<UiLanguage>(() => (localStorage.getItem(UI_LANGUAGE_KEY) || "system") as UiLanguage);
  useEffect(() => {
    document.documentElement.classList.add("computer-notice-page");
    const changed = (event: StorageEvent) => { if (event.key === UI_LANGUAGE_KEY) setLanguage((event.newValue || "system") as UiLanguage); };
    window.addEventListener("storage", changed);
    return () => { document.documentElement.classList.remove("computer-notice-page"); window.removeEventListener("storage", changed); };
  }, []);
  useEffect(() => {
    let disposed = false, received = false;
    const stops: (() => void)[] = [];
    const attach = async () => {
      const stateStop = await listen<ComputerUseActivity>("computer-use-activity", event => {
        received = true;
        if (!disposed) { setActivity(event.payload); setDisconnected(false); }
      });
      if (disposed) { stateStop(); return; } stops.push(stateStop);
      const lostStop = await listen("computer-use-disconnected", () => { received = true; if (!disposed) setDisconnected(true); });
      if (disposed) { lostStop(); return; } stops.push(lostStop);
      const initial = await invoke<ComputerUseActivity>("core_computer_notice", { call:{type:"status"} });
      if (!disposed && !received) setActivity(initial);
    };
    void attach().catch(() => { if (!disposed) setDisconnected(true); });
    return () => { disposed = true; stops.forEach(stop => stop()); };
  }, []);
  useEffect(() => {
    if (disconnected || activity.mode !== "foreground" || !activity.preparing || !activity.operationId) return;
    let second = 0;
    // Backend has shown the non-activating window. ACK only after the warning
    // has had an opportunity to paint; Core adds a one-second notice period.
    const first = requestAnimationFrame(() => { second = requestAnimationFrame(() => {
      void invoke("core_computer_notice", { call:{type:"presented", operationId:activity.operationId} }).catch(() => setDisconnected(true));
    }); });
    return () => { cancelAnimationFrame(first); cancelAnimationFrame(second); };
  }, [activity.operationId, activity.preparing, activity.mode, disconnected]);
  return <I18nProvider language={language}><ComputerUseNoticeView activity={activity} disconnected={disconnected} captionTop={captionInset()} /></I18nProvider>;
}

export function ComputerUseNoticeView({ activity, disconnected = false, captionTop = 0 }: { activity: ComputerUseActivity; disconnected?: boolean; captionTop?: number }) {
  const { t } = useI18n();
  const foreground = activity.mode === "foreground" || disconnected;
  const idle = activity.mode === "idle" && !disconnected;
  const message = disconnected ? t("computerUse.glowDisconnected") : foreground
    ? t(activity.foregroundPossible ? "computerUse.glowPossible" : "computerUse.glowForeground")
    : t("computerUse.glowBackground");
  return <aside className={`computer-glow${foreground ? " computer-glow-foreground" : ""}${idle ? " computer-glow-idle" : ""}${captionTop > 0 ? " computer-glow-safe-area" : ""}`} style={{"--computer-caption-top": `${captionTop}px`} as CSSProperties} role={foreground ? "alert" : "status"} aria-hidden={idle}>
    <div className="computer-glow-haze" aria-hidden="true" />
    <div className="computer-glow-edge" aria-hidden="true" />
    <div className="computer-glow-caption"><span>{message}</span>{activity.mode === "foreground" && <span className="computer-glow-escape">Esc {t("computerUse.glowExit")}</span>}</div>
  </aside>;
}
