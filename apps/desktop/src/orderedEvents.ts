import type { EventEnvelope } from "@dogcodes/protocol";

export type CursorResult = "applied" | "duplicate" | "gap";

/** Tracks the global Canonical Event sequence independently of React state. */
export class OrderedEventCursor {
  constructor(private sequence: number) {}

  get lastSequence() {
    return this.sequence;
  }

  accept(event: EventEnvelope): CursorResult {
    if (event.sequence <= this.sequence) return "duplicate";
    if (event.sequence !== this.sequence + 1) return "gap";
    this.sequence = event.sequence;
    return "applied";
  }
}

export function applyContiguous(
  cursor: OrderedEventCursor,
  events: EventEnvelope[],
): { applied: number; gapAt: number | null } {
  let applied = 0;
  for (const event of events) {
    const result = cursor.accept(event);
    if (result === "gap") return { applied, gapAt: event.sequence };
    if (result === "applied") applied += 1;
  }
  return { applied, gapAt: null };
}

