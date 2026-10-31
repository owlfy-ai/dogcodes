import { useEffect, useState } from "react";
import type { CoreClient, EventEnvelope, EventStreamMessage, ReviewActivity, TurnReviewRun } from "@dogcodes/protocol";
import { translate as t } from "./i18n";

export const reviewActive = (run: TurnReviewRun) => ["queued", "running", "cancelling"].includes(run.status);
type Listener = (event: EventEnvelope) => void;
// One event subscription per client, shared by all visible turn/project reviews.
const streams = new WeakMap<CoreClient, { listeners: Set<Listener>; dispose: () => void }>();
function listen(client: CoreClient, listener: Listener) {
  let stream = streams.get(client);
  if (!stream) {
    const listeners = new Set<Listener>();
    let disposed = false, stop: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cursor: number | undefined;
    let retry = 500;
    let processing = Promise.resolve();
    const reconnect = () => {
      if (disposed || timer) return;
      timer = setTimeout(() => { timer = undefined; void connect(); }, retry);
      retry = Math.min(retry * 2, 8000);
    };
    const publish = (event: EventEnvelope) => {
      if (disposed || event.sequence <= (cursor ?? 0)) return;
      if (event.sequence !== (cursor ?? 0) + 1) throw new Error(t("reviewStream.gap"));
      cursor = event.sequence;
      if (["review.started", "review.updated", "review.activity"].includes(event.eventType)) {
        listeners.forEach(fn => fn(event));
      }
    };
    const receive = (message: EventStreamMessage) => {
      processing = processing.then(async () => {
        if (disposed) return;
        if (message.type === "error") { if (message.data.retryable) reconnect(); return; }
        if (message.type !== "event" || message.data.sequence <= (cursor ?? 0)) return;
        if (message.data.sequence > (cursor ?? 0) + 1) {
          const missing = await client.syncEvents(cursor ?? 0);
          missing.forEach(publish);
        }
        publish(message.data);
      }).catch(reconnect);
    };
    const connect = async () => {
      try {
        cursor ??= (await client.bootstrap()).lastSequence;
        if (disposed) return;
        stop?.(); stop = undefined;
        const unsubscribe = await client.subscribe(cursor, receive);
        if (disposed) unsubscribe(); else { stop = unsubscribe; retry = 500; }
      } catch { reconnect(); }
    };
    stream = { listeners, dispose: () => { disposed = true; clearTimeout(timer); stop?.(); } };
    streams.set(client, stream);
    void connect();
  }
  stream.listeners.add(listener);
  return () => {
    stream.listeners.delete(listener);
    if (!stream.listeners.size) { stream.dispose(); streams.delete(client); }
  };
}

function merge(items: ReviewActivity[], updates: ReviewActivity[]) {
  const merged = new Map(items.map(item => [item.itemId, item]));
  updates.forEach(item => merged.set(item.itemId, item));
  return [...merged.values()];
}

/** Persisted polling remains the fallback; stream snapshots update only this review. */
export function useReviewStream(client: CoreClient, run: TurnReviewRun): TurnReviewRun {
  const [live, setLive] = useState<{ id: string; revision: number; activities: ReviewActivity[]; finished?: TurnReviewRun }>();
  const id = run.request.runId, taskId = run.request.taskId;
  const active = reviewActive(run);
  useEffect(() => {
    if (!active || !client.subscribe || !client.bootstrap) return;
    return listen(client, event => {
      const payload = event.payload as { taskId?: string; runId?: string; revision?: number; activities?: ReviewActivity[]; review?: TurnReviewRun };
      if (payload.taskId !== taskId || (payload.runId ?? payload.review?.request.runId) !== id) return;
      const full = payload.review;
      const revision = full?.activityRevision ?? payload.revision ?? 0;
      const activities = full?.activities ?? payload.activities ?? [];
      setLive(previous => {
        if (previous?.id === id && previous.revision > revision) return previous;
        return { id, revision, activities: merge(previous?.id === id ? previous.activities : [], activities),
          finished: full && !reviewActive(full) ? full : previous?.id === id ? previous.finished : undefined };
      });
    });
  }, [client, id, taskId, active]);
  if (!live || live.id !== id || live.revision < (run.activityRevision ?? 0)) return run;
  const base = active && live.finished ? { ...live.finished, repairs: run.repairs } : run;
  return { ...base, activityRevision: live.revision, activities: merge(base.activities ?? [], live.activities) };
}
