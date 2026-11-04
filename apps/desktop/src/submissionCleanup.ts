import { useCallback, useEffect, useRef, useSyncExternalStore } from "react";
import type { CoreClient } from "@dogcodes/protocol";
import { submissionOperations as operations, type PendingSubmission, type SubmissionResult, type UnconfirmedSubmission, type UnconfirmedCheck } from "./submissionOperations";

export function usePendingSubmissions(client: CoreClient) {
  useEffect(() => operations.watch(client), [client]);
  const snapshot = useCallback(() => operations.snapshot(client), [client]);
  return JSON.parse(useSyncExternalStore(operations.subscribe, snapshot, () => "[]")) as PendingSubmission[] | { error: string; records: PendingSubmission[] };
}
/** Sends whose outcome is unknown on this client, kept with their original identity. */
export function useUnconfirmedSubmissions() {
  return JSON.parse(useSyncExternalStore(operations.subscribe, operations.unconfirmed, () => '{"records":[],"error":null}')) as { records: UnconfirmedSubmission[]; error: string | null };
}

// Long enough for a healthy send from another window to finish before it is stopped.
const stuckAfterMs = 2 * 60_000;
const retryMs = 15_000;

interface Cleanup {
  client: CoreClient;
  busy: boolean;
  /** Only the open conversation's sends: their text can go back to its composer. */
  pending: PendingSubmission[];
  unconfirmed: UnconfirmedSubmission[];
  onSettled: (result: SubmissionResult | Exclude<UnconfirmedCheck, { status: "not_received" | "pending" }>, operationId: string) => void;
  /** The conversation may have changed while the service answered; the identity tells. */
  onRestore: (prompt: string, origin: { taskId: string | null; scope: string }) => void;
  onError: (cause: unknown) => void;
}

/**
 * Stuck sends block the conversation, so they are ended here without asking: the
 * service stops the original, and text it never accepted returns to the composer.
 */
export function useStuckSubmissionCleanup(options: Cleanup) {
  const latest = useRef(options);
  latest.current = options;
  const running = useRef(false);
  const { client, busy, pending, unconfirmed } = options;
  const blocked = [...pending, ...unconfirmed].map(record => record.operationId).join();
  useEffect(() => {
    if (busy || !blocked) return;
    // A finished stop removes the record and re-runs this effect, so nothing below
    // may depend on this run still being current: the text would be lost.
    const run = async () => {
      if (running.current) return;
      running.current = true;
      const current = latest.current;
      try {
        for (const record of current.unconfirmed) {
          const result = await operations.verifyUnconfirmed(client, record);
          if (result.status === "not_received") {
            operations.abandonUnconfirmed(record);
            current.onRestore(record.input.prompt, { taskId: record.taskId, scope: record.scope });
          } else if (result.status !== "pending") current.onSettled(result, record.operationId);
        }
        for (const record of current.pending) {
          if (Date.now() - Date.parse(record.requestedAt) < stuckAfterMs) continue;
          const result = await operations.retire(client, record);
          if (result.status === "pending") continue;
          if (result.status === "invalidated") current.onRestore(record.input.prompt, { taskId: record.task.taskId, scope: record.scope });
          current.onSettled(result, record.operationId);
        }
      } catch (cause) { current.onError(cause); }
      finally { running.current = false; }
    };
    void run();
    const timer = window.setInterval(() => void run(), retryMs);
    return () => clearInterval(timer);
  }, [client, busy, blocked]);
}
