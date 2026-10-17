import { describe, expect, it } from "vitest";
import type { EventEnvelope } from "@dogcodes/protocol";
import { OrderedEventCursor, applyContiguous } from "./orderedEvents";

const event = (sequence: number): EventEnvelope => ({
  schemaVersion: 1,
  sequence,
  eventId: `evt_${sequence}`,
  stream: { streamType: "task", streamId: "tsk_test" },
  streamVersion: sequence,
  eventType: "test.event",
  occurredAt: "2026-09-02T00:00:00Z",
  recordedAt: "2026-09-02T00:00:00Z",
  correlationId: "cor_test",
  causationId: null,
  source: { kind: "hub" },
  payload: {},
  vendorExtensions: null,
});

describe("OrderedEventCursor", () => {
  it("applies continuous events and ignores replay/live duplicates", () => {
    const cursor = new OrderedEventCursor(4);
    expect(cursor.accept(event(5))).toBe("applied");
    expect(cursor.accept(event(5))).toBe("duplicate");
    expect(cursor.accept(event(6))).toBe("applied");
    expect(cursor.lastSequence).toBe(6);
  });

  it("pauses at a gap without advancing the cursor", () => {
    const cursor = new OrderedEventCursor(10);
    expect(cursor.accept(event(12))).toBe("gap");
    expect(cursor.lastSequence).toBe(10);
    expect(applyContiguous(cursor, [event(11), event(12)])).toEqual({
      applied: 2,
      gapAt: null,
    });
  });
});

