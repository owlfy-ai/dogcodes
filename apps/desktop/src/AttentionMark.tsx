import { attentionLabel, type AttentionKind } from "./conversationAttention";

const GLYPH: Record<AttentionKind, string> = { approval: "!", question: "?", failed: "×", completed: "" };

/** Shape as well as color, so the states stay apart for anyone who can't tell the colors. */
export function AttentionMark({ kind }: { kind: AttentionKind }) {
  const label = attentionLabel(kind);
  return <span className="task-attention" data-kind={kind} role="img" aria-label={label} data-tooltip={label}><span aria-hidden="true">{GLYPH[kind]}</span></span>;
}
