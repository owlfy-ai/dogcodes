// Keep the latest streamed text at the right edge. Each incoming chunk pushes
// preceding text left; no timer, replay, or scrolling while the stream is idle.
export function ThinkingTicker({ text }: { text: string }) {
  return (
    <div className="thought-marquee" tabIndex={0} aria-label="正在思考">
      <span aria-hidden="true">{text}</span>
    </div>
  );
}
