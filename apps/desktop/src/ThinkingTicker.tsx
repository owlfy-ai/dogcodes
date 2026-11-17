import { translate as t } from "./i18n";
// Keep the latest streamed text at the right edge. Each incoming chunk pushes
// preceding text left; no timer, replay, or scrolling while the stream is idle.
export function ThinkingTicker({ text, embedded = false }: { text: string; embedded?: boolean }) {
  return (
    <span className="thought-marquee" tabIndex={embedded ? undefined : 0} aria-label={t("activity.thinking")}>
      <span aria-hidden="true">{text}</span>
    </span>
  );
}
