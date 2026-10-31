import { translate as t } from "./i18n";
// Keep the latest streamed text at the right edge. Each incoming chunk pushes
// preceding text left; no timer, replay, or scrolling while the stream is idle.
export function ThinkingTicker({ text }: { text: string }) {
  return (
    <div className="thought-marquee" tabIndex={0} aria-label={t("activity.thinking")}>
      <span aria-hidden="true">{text}</span>
    </div>
  );
}
