import { useEffect, useRef, useState } from "react";

const segmenter = new Intl.Segmenter(undefined, { granularity: "grapheme" });
const CHARACTER_MS = 12;
const MAX_DELAY_MS = 160;

/** Presentation only: canonical text and event cursors never wait for animation. */
export class TextReveal {
  text: string;
  shown: number;
  private pending: { end: number; at: number }[] = [];
  private lastBoundary = 0;

  constructor(text: string) { this.text = text; this.shown = text.length; this.reset(text); }

  reset(text: string) {
    this.text = text; this.shown = text.length; this.pending = [];
    this.lastBoundary = 0;
    for (const part of segmenter.segment(text)) this.lastBoundary = part.index;
    return text;
  }

  append(text: string, now: number) {
    if (!text.startsWith(this.text)) { this.reset(text); return; }
    if (text === this.text) return;
    // Resegment the last grapheme too: a later chunk may extend an emoji or
    // combining sequence. Never reveal a partial grapheme from the known text.
    const previous = this.lastBoundary;
    const previousLast = this.pending.at(-1);
    const retained = this.pending.filter(part => part.end <= previous);
    let at = Math.max(now, retained.at(-1)?.at ?? now);
    for (const part of segmenter.segment(text.slice(previous))) {
      this.lastBoundary = previous + part.index;
      const end = previous + part.index + part.segment.length;
      if (end <= this.shown) continue;
      at = previousLast && previous + part.index === previous
        ? Math.max(now, previousLast.at) : at + CHARACTER_MS;
      retained.push({ end, at });
    }
    this.pending = retained;
    this.text = text;
    const duration = at - now;
    if (duration > MAX_DELAY_MS) {
      for (const part of this.pending) part.at = now + Math.max(0, part.at - now) * MAX_DELAY_MS / duration;
    }
  }

  frame(now: number) {
    let consumed = 0;
    while (consumed < this.pending.length && this.pending[consumed].at <= now) {
      this.shown = this.pending[consumed++].end;
    }
    if (consumed) this.pending.splice(0, consumed);
    return this.text.slice(0, this.shown);
  }

  get waiting() { return this.pending.length > 0; }
}

export function useSmoothText(text: string, streaming: boolean): string {
  // A mounted snapshot is already available history, not a new token burst.
  const reveal = useRef<TextReveal | null>(null);
  if (!reveal.current) reveal.current = new TextReveal(text);
  const [visible, setVisible] = useState(text);
  const [reducedMotion, setReducedMotion] = useState(() =>
    typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches);

  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const media = matchMedia("(prefers-reduced-motion: reduce)");
    const changed = () => setReducedMotion(media.matches);
    changed(); media.addEventListener("change", changed);
    return () => media.removeEventListener("change", changed);
  }, []);

  useEffect(() => {
    const state = reveal.current!;
    let frame: number | undefined;
    const flush = () => {
      if (frame !== undefined) cancelAnimationFrame(frame);
      frame = undefined;
      setVisible(state.reset(text));
    };
    if (!streaming || reducedMotion || document.hidden || typeof requestAnimationFrame !== "function") {
      flush(); return;
    }
    state.append(text, performance.now());
    setVisible(state.frame(performance.now()));
    const tick = (now: number) => {
      frame = undefined;
      setVisible(state.frame(now));
      if (state.waiting) frame = requestAnimationFrame(tick);
    };
    if (state.waiting) frame = requestAnimationFrame(tick);
    document.addEventListener("visibilitychange", flush);
    return () => {
      if (frame !== undefined) cancelAnimationFrame(frame);
      document.removeEventListener("visibilitychange", flush);
    };
  }, [text, streaming, reducedMotion]);

  return !streaming || reducedMotion || !text.startsWith(visible) ? text : visible;
}
