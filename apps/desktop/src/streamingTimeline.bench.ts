// pnpm --filter @dogcodes/desktop exec vitest bench src/streamingTimeline.bench.ts --run
import { bench, describe } from "vitest";
import type { EventEnvelope, TimelineItem } from "@dogcodes/protocol";
import { applyTimelineBatch } from "./streamingTimeline";

const items = Array.from({ length: 1000 }, (_, index) => ({
  itemId: `message:tsk_bench:trn_bench:${index}`, taskId: "tsk_bench", turnId: "trn_bench",
  kind: "assistant_message", text: "历史正文", attachments: [], providerId: "codex",
  sequence: index + 1, occurredAt: "2026-09-29T00:00:00Z",
})) as TimelineItem[];
const events = Array.from({ length: 300 }, (_, index) => ({
  schemaVersion: 1, sequence: 1001 + index, eventId: `evt_${index}`, stream: { streamType: "task", streamId: "tsk_bench" },
  streamVersion: 1001 + index, eventType: "message.delta", occurredAt: "2026-09-29T00:00:00Z",
  recordedAt: "2026-09-29T00:00:00Z", correlationId: "cor_bench", causationId: null,
  source: { kind: "driver", providerId: "codex" }, vendorExtensions: null,
  payload: { taskId: "tsk_bench", turnId: "trn_bench", itemId: "999", text: "新增文字" },
})) as EventEnvelope[];

// Previous message.delta algorithm, limited to the valid existing message in
// this fixture. Excludes validation, making this a conservative baseline.
function previousProjection() {
  let next = items;
  for (const event of events) {
    const payload = event.payload as { taskId: string; turnId: string; itemId: string; text: string };
    const id = `message:${payload.taskId}:${payload.turnId}:${payload.itemId}`;
    const index = next.findIndex(item => item.itemId === id);
    const updated = { ...next[index], text: next[index].text + payload.text };
    next = next.slice(); next[index] = updated;
  }
  return next;
}

describe("1000 rows / 300 message deltas (projection only)", () => {
  bench("previous per-event search and array copy", () => { previousProjection(); });
  bench("one indexed batch with joined text", () => { applyTimelineBatch(items, events); });
});
