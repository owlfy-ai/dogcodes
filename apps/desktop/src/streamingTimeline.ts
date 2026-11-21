import type { EventEnvelope, TaskId, TimelineItem, TimelineSnapshot, TurnId } from "@dogcodes/protocol";

export const isTimelineDelta = (event: EventEnvelope) => ["message.delta", "activity.updated", "tool.started", "tool.completed", "computer_use.action"].includes(event.eventType);
const terminal = new Set(["completed", "failed", "interrupted", "cancelled"]);

function delta(event: EventEnvelope) {
  if (!isTimelineDelta(event) || event.stream.streamType !== "task") return null;
  const payload = event.payload as Record<string, unknown>;
  if (!payload || typeof payload.turnId !== "string" || payload.taskId !== event.stream.streamId) return null;
  const message = event.eventType === "message.delta", activity = event.eventType === "activity.updated", computer = event.eventType === "computer_use.action";
  if ((message || activity) && typeof payload.itemId !== "string") return null;
  if (message && typeof payload.text !== "string") return null;
  if (activity && (!payload.data || typeof payload.data !== "object" || Array.isArray(payload.data))) return null;
  if (!message && !activity && !computer && typeof payload.toolName !== "string") return null;
  if (computer && typeof payload.tool !== "string") return null;
  const taskId = event.stream.streamId as TaskId, turnId = payload.turnId as TurnId;
  const itemId = !computer && typeof payload.itemId === "string"
    ? `${message ? "message" : "tool"}:${taskId}:${turnId}:${payload.itemId}` : event.eventId;
  return { payload, message, activity, computer, taskId, turnId, itemId };
}

/** One index and at most one array/row copy per batch. Untouched rows keep their
 * identity. Activity JSON is decoded/encoded once per changed row. */
export function applyTimelineBatch(items: TimelineItem[], events: readonly EventEnvelope[]): { items: TimelineItem[]; dirty: boolean } {
  if (!events.length) return { items, dirty: false };
  let next = items, dirty = false;
  // Most batches only update the tail. Index older rows only when one is addressed.
  let positions: Map<string, number> | undefined;
  const position = (id: string) => {
    if (next.at(-1)?.itemId === id) return next.length - 1;
    positions ??= new Map(next.map((item, index) => [item.itemId, index]));
    return positions.get(id);
  };
  const changed = new Set<string>();
  const activities = new Map<string, Record<string, unknown>>();
  const messages = new Map<string, string[]>();
  for (const event of events) {
    const entry = delta(event);
    if (!entry) { dirty = true; continue; }
    const { payload, message, activity, computer, taskId, turnId, itemId } = entry;
    const index = position(itemId), previous = index === undefined ? undefined : next[index];
    if (!message && !activity && previous?.kind === "activity") continue;
    let data = activities.get(itemId);
    if (activity && !data) {
      try { data = previous?.kind === "activity" ? JSON.parse(previous.text) : {}; } catch { dirty = true; continue; }
      if (!data || typeof data !== "object" || Array.isArray(data)) { dirty = true; continue; }
      activities.set(itemId, data);
    }
    if (activity) {
      const fields = data!;
      let modified = false;
      for (const [key, value] of Object.entries(payload.data as Record<string, unknown>)) {
        if (value == null) continue;
        // The core already used the native ID for deduplication. It has no visible content.
        if (key === "sourceEventId" && (payload.data as Record<string, unknown>).type === "thought") continue;
        if (key === "type" && fields.type === "subagent" && value === "tool") continue;
        if (key === "parentId" && value === "" && fields.parentId) continue;
        if (key === "startedAt" && fields.startedAt != null) continue;
        if (key === "status" && terminal.has(String(fields.status)) && ["pending", "running", "in_progress"].includes(String(value))) continue;
        if (key !== "textDelta" && fields[key] === value) continue;
        modified = true;
        if (key === "textDelta") fields.text = String(fields.text ?? "") + String(value);
        else fields[key] = value;
      }
      if (!modified && previous?.kind === "activity") continue;
    }
    const item = changed.has(itemId) ? previous! : previous ? { ...previous } : {
      itemId, taskId, turnId, kind: "", text: "", attachments: [],
      providerId: event.source.kind === "driver" ? event.source.providerId ?? null : null,
      sequence: event.sequence, occurredAt: event.occurredAt,
    };
    if (!changed.has(itemId)) {
      if (next === items) next = items.slice();
      if (index === undefined) { positions?.set(itemId, next.length); next.push(item); }
      else next[index] = item;
      changed.add(itemId);
    }
    if (computer) {
      item.kind = "computer_use";
      item.text = JSON.stringify(payload);
      item.providerId = typeof payload.providerId === "string" ? payload.providerId : null;
    } else if (message) {
      item.kind = "assistant_message";
      let chunks = messages.get(itemId);
      if (!chunks) { chunks = [item.text]; messages.set(itemId, chunks); }
      chunks.push(payload.text as string);
    } else if (activity) {
      item.kind = "activity";
    } else {
      const keepSummary = item.kind === "tool.started" && event.eventType === "tool.completed";
      item.kind = event.eventType === "tool.completed" && payload.success === false ? "tool.failed" : event.eventType;
      if (!keepSummary) item.text = typeof payload.summary === "string" ? payload.summary
        : `${event.eventType === "tool.started" ? "Started" : "Completed"} ${payload.toolName}`;
      item.providerId = event.source.kind === "driver" ? event.source.providerId ?? null : null;
    }
  }
  for (const [id, chunks] of messages) next[position(id)!].text = chunks.join("");
  for (const [id, data] of activities) {
    if (changed.has(id)) next[position(id)!].text = JSON.stringify(data);
  }
  return { items: next, dirty };
}

export function applyTimelineDelta(items: TimelineItem[], event: EventEnvelope): TimelineItem[] | null {
  const result = applyTimelineBatch(items, [event]);
  return result.dirty ? null : result.items;
}

type Read = { events: EventEnvelope[] };

/** Receive in order without projecting each event. Flush once per UI batch.
 * Reads retain their own events so a snapshot can replace even an unflushed batch. */
export class StreamingTimeline {
  items: TimelineItem[] | null = null;
  private through = 0;
  private pending: EventEnvelope[] = [];
  private reads = new Set<Read>();
  beginRead(): Read { const read = { events: [...this.pending] }; this.reads.add(read); return read; }
  endRead(read: Read) { this.reads.delete(read); }
  receive(event: EventEnvelope): boolean {
    if (event.sequence <= this.through) return true;
    for (const read of this.reads) read.events.push(event);
    this.through = event.sequence;
    if (!delta(event)) return false;
    this.pending.push(event);
    return this.items !== null;
  }
  flush(): { items: TimelineItem[] | null; dirty: boolean } {
    const events = this.pending; this.pending = [];
    if (!this.items) return { items: null, dirty: events.length > 0 };
    const result = applyTimelineBatch(this.items, events);
    this.items = result.items;
    return result;
  }
  replace(read: Read, snapshot: TimelineSnapshot): { items: TimelineItem[]; dirty: boolean } {
    this.endRead(read);
    let through = snapshot.lastSequence;
    const events = read.events.filter(event => {
      if (event.sequence <= through) return false;
      through = event.sequence; return true;
    });
    const result = applyTimelineBatch(snapshot.items, events);
    this.items = result.items;
    this.pending = [];
    this.through = through;
    return result;
  }
}
