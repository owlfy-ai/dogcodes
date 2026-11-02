import type { TimelineItem } from "@dogcodes/protocol";
import { ActivityIcon } from "./ActivityIcon";
import { useI18n } from "./i18n";

export function ComputerUseActivity({ item }: { item: TimelineItem }) {
  const { t } = useI18n();
  let action: Record<string, unknown>;
  try { action = JSON.parse(item.text); } catch { return null; }
  if (!action || typeof action !== "object" || typeof action.tool !== "string") return null;
  const mode = ["observing", "background", "foreground"].includes(String(action.mode)) ? String(action.mode) : "unknown";
  const outcome = action.transportOk === false ? "unconfirmed" : action.isError === true ? "failed" : action.effect === "unverifiable" ? "unverified" : "succeeded";
  const elapsed = typeof action.elapsedMs === "number" && Number.isFinite(action.elapsedMs) ? Math.max(0, action.elapsedMs) : 0;
  const images = typeof action.imageCount === "number" ? Math.max(0, action.imageCount) : 0;
  return <details className="conversation-activity">
    <summary className="activity-line">
      <span className="activity-label"><ActivityIcon kind="computer" /><span>{t("computerUse.action")}</span></span>
      <span className="activity-title">{t(`computerUse.tools.${action.tool}`)}</span>
      <small>{t(`computerUse.${mode}`)} · {t(`computerUse.${outcome}`)}</small>
    </summary>
    <dl className="activity-detail computer-use-detail">
      <dt>{t("computerUse.duration")}</dt><dd>{(elapsed / 1000).toFixed(2)} s</dd>
      <dt>{t("computerUse.images")}</dt><dd>{images}</dd>
      {action.foregroundPossible === true && <><dt>{t("computerUse.foreground")}</dt><dd>{t("computerUse.glowPossible")}</dd></>}
    </dl>
  </details>;
}
