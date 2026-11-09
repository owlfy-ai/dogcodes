import { useState } from "react";
import { useLatestComputerUsePreview } from "./computerUsePreviews";
import { useI18n } from "./i18n";

/** Newest computer-use screenshot, floating over the conversation while the turn runs. */
export function ComputerUsePreviewOverlay({ taskId, turnId }: { taskId?: string; turnId?: string }) {
  const { t } = useI18n();
  const preview = useLatestComputerUsePreview(taskId, turnId);
  const [expanded, setExpanded] = useState(false);
  if (!preview?.image) return null;
  const marker = preview.marker;
  const label = preview.element ? `${preview.window ?? ""}${preview.window ? " · " : ""}${preview.element}` : preview.window;
  return <figure className={`computer-use-overlay${expanded ? " expanded" : ""}`}>
    <button type="button" className="computer-use-frame" aria-expanded={expanded}
      aria-label={expanded ? t("computerUse.previewShrink") : t("computerUse.previewEnlarge")}
      data-tooltip={label} onClick={() => setExpanded(value => !value)}>
      <img src={preview.image} alt={preview.window ?? t("computerUse.previewAlt")} draggable={false} />
      {marker && <span className="computer-use-marker" aria-label={t("computerUse.clickPoint")}
        style={{ left: `${marker.x * 100}%`, top: `${marker.y * 100}%` }} />}
    </button>
    {expanded && label && <figcaption>{label}</figcaption>}
  </figure>;
}
