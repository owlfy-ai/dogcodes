import { expect, it } from "vitest";
import type { EventEnvelope, TimelineItem } from "@dogcodes/protocol";
import { applyTimelineBatch, applyTimelineDelta, StreamingTimeline } from "./streamingTimeline";
const event = (sequence: number, eventType = "message.delta", payload: Record<string, unknown> = {}): EventEnvelope => ({
  schemaVersion: 1, sequence, eventId: `evt_${sequence}`, stream: { streamType: "task", streamId: "tsk_one" },
  streamVersion: sequence, eventType, occurredAt: "2026-09-28T00:00:00Z", recordedAt: "2026-09-28T00:00:00Z",
  correlationId: "cor_test", causationId: null, source: { kind: "driver", providerId: "cursor" }, vendorExtensions: null,
  payload: { taskId: "tsk_one", turnId: "trn_one", itemId: "answer", text: "你", ...payload },
});
const seed = (stream: StreamingTimeline) => stream.replace(stream.beginRead(), { items: [], lastSequence: 0 });
it("keeps independent computer actions when replaying a snapshot during streaming", () => {
  const stream = new StreamingTimeline(); seed(stream);
  const first = event(1, "computer_use.action", { tool: "click", providerId: "codex", mode: "background" });
  stream.receive(first);
  const read = stream.beginRead();
  stream.receive(event(2, "computer_use.action", {tool:"get_window_state",providerId:"codex"}));
  stream.replace(read, { items: [{itemId:first.eventId,kind:"computer_use",text:JSON.stringify(first.payload)} as TimelineItem],lastSequence:1 });
  expect(stream.items?.map(item=>item.itemId)).toEqual(["evt_1","evt_2"]);
  expect(stream.items?.[1].providerId).toBe("codex");
  expect(stream.items?.[1].kind).toBe("computer_use");
});
it("appends deltas while preserving unrelated rows and their identities", () => {
  const old = { itemId: "old", text: "earlier" } as TimelineItem;
  const first = applyTimelineDelta([old], event(1))!;
  const next = applyTimelineDelta(first, event(2, "message.delta", { text: "好" }))!;
  expect(next[0]).toBe(old); expect(next[1].text).toBe("你好"); expect(next[1].sequence).toBe(1);
  expect(first[1].text).toBe("你");
});
it("replays only deltas beyond the snapshot cursor, without losing or duplicating text", () => {
  const stream = new StreamingTimeline(); seed(stream);
  stream.receive(event(1));
  const read = stream.beginRead();
  stream.receive(event(2, "message.delta", { text: "好" }));
  stream.flush();
  const captured = stream.items!;
  stream.receive(event(3, "message.delta", { text: "！" }));
  expect(stream.replace(read, { items: captured, lastSequence: 2 }).items[0].text).toBe("你好！");
  stream.receive(event(3, "message.delta", { text: "！" }));
  expect(stream.items![0].text).toBe("你好！");
});

it("projects a burst once, preserves old snapshots and leaves unrelated rows unchanged", () => {
  const stream = new StreamingTimeline();
  const old = { itemId: "old", text: "earlier" } as TimelineItem;
  stream.replace(stream.beginRead(), { items: [old], lastSequence: 0 });
  const before = stream.items;
  for (let sequence = 1; sequence <= 300; sequence++) stream.receive(event(sequence));
  expect(stream.items).toBe(before);
  const result = stream.flush();
  expect(result.dirty).toBe(false);
  expect(result.items![0]).toBe(old);
  expect(result.items![1]).toMatchObject({ text: "你".repeat(300), sequence: 1 });
  expect(before).toEqual([old]);
  expect(stream.flush().items).toBe(result.items);
  stream.receive(event(300));
  expect(stream.flush().items).toBe(result.items);
});

it("replaces a snapshot before the scheduled flush without replaying queued text twice", () => {
  const stream = new StreamingTimeline(); seed(stream);
  stream.receive(event(1));
  const read = stream.beginRead();
  stream.receive(event(2, "message.delta", { text: "好" }));
  const snapshotItems = applyTimelineDelta([], event(1))!;
  const result = stream.replace(read, { items: snapshotItems, lastSequence: 1 });
  expect(result.items[0].text).toBe("你好");
  expect(stream.flush().items).toBe(result.items);
  expect(snapshotItems[0].text).toBe("你");
});

it("includes pending events at read start and continues after a structural or malformed event", () => {
  const stream = new StreamingTimeline(); seed(stream);
  stream.receive(event(1));
  const read = stream.beginRead();
  stream.receive(event(2, "turn.completed"));
  stream.receive(event(3, "message.delta", { text: 42 }));
  stream.receive(event(4, "message.delta", { text: "好" }));
  const result = stream.replace(read, { items: [], lastSequence: 0 });
  expect(result.items[0].text).toBe("你好");
  expect(result.dirty).toBe(true);
  expect(stream.flush().items).toBe(result.items);
});

it("merges mixed message/activity/tool events in order within one batch", () => {
  const result = applyTimelineBatch([], [
    event(1),
    event(2, "tool.started", { itemId: "tool", toolName: "Read", summary: "读取文件" }),
    event(3, "activity.updated", { itemId: "child", data: { type: "subagent", textDelta: "a", startedAt: "first", status: "running" } }),
    event(4, "message.delta", { text: "好" }),
    event(5, "activity.updated", { itemId: "child", data: { type: "tool", textDelta: "b", status: "completed" } }),
    event(6, "tool.completed", { itemId: "tool", toolName: "Read", success: false, summary: "失败" }),
    event(7, "activity.updated", { itemId: "child", data: { startedAt: "later", text: null, status: "running" } }),
    event(8, "tool.completed", { itemId: "child", toolName: "Task" }),
  ]);
  expect(result.dirty).toBe(false);
  expect(result.items.map(item => item.sequence)).toEqual([1, 2, 3]);
  expect(result.items[0].text).toBe("你好");
  expect(result.items[1]).toMatchObject({ kind: "tool.failed", text: "读取文件" });
  expect(JSON.parse(result.items[2].text)).toEqual({ type: "subagent", text: "ab", startedAt: "first", status: "completed" });
});

it("flags corrupt activity state while still applying valid rows in the batch", () => {
  const broken = { itemId: "tool:tsk_one:trn_one:bad", kind: "activity", text: "not JSON" } as TimelineItem;
  const result = applyTimelineBatch([broken], [
    event(1, "activity.updated", { itemId: "bad", data: { textDelta: "more" } }), event(2),
  ]);
  expect(result.dirty).toBe(true);
  expect(result.items[0]).toBe(broken);
  expect(result.items[1].text).toBe("你");
});
it("buffers before initial history is ready and flags structural events for canonical reload", () => {
  const stream = new StreamingTimeline(), read = stream.beginRead();
  stream.receive(event(1)); stream.receive(event(2, "turn.completed"));
  const result = stream.replace(read, { items: [], lastSequence: 0 });
  expect(result.items[0].text).toBe("你"); expect(result.dirty).toBe(true);
});
it("handles overlapping reads when the caller discards an obsolete response", () => {
  const stream = new StreamingTimeline(); seed(stream);
  const obsolete = stream.beginRead(), latest = stream.beginRead();
  stream.receive(event(1)); stream.endRead(obsolete);
  expect(stream.replace(latest, { items: [], lastSequence: 0 }).items[0].text).toBe("你");
});
it("matches activity merge semantics including textDelta, nulls, subagents and terminal state", () => {
  let items = applyTimelineDelta([], event(1, "activity.updated", { data: { type: "subagent", textDelta: "a", parentId: "p", startedAt: "first", status: "completed" } }))!;
  items = applyTimelineDelta(items, event(2, "activity.updated", { data: { type: "tool", textDelta: "b", parentId: "", startedAt: "later", status: "running", text: null } }))!;
  expect(JSON.parse(items[0].text)).toEqual({ type: "subagent", text: "ab", parentId: "p", startedAt: "first", status: "completed" });
  expect(applyTimelineDelta(items, event(3, "tool.completed", { toolName: "Task", summary: "done" }))).toBe(items);
});
it("keeps tool start summaries and marks failed completion", () => {
  const items = applyTimelineDelta([], event(1, "tool.started", { toolName: "Read", summary: "读取文件" }))!;
  expect(applyTimelineDelta(items, event(2, "tool.completed", { toolName: "Read", success: false, summary: "失败" }))![0]).toMatchObject({ kind: "tool.failed", text: "读取文件", sequence: 1 });
});
