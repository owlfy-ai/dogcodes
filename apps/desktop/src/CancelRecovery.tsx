import { useEffect, useRef, useSyncExternalStore } from "react";
import type { CoreClient, TaskId } from "@dogcodes/protocol";
import { cancelOperations as operations, type PendingCancellation } from "./cancelOperations";

export function usePendingCancellations(taskId: TaskId | null) {
  const saved = useSyncExternalStore(operations.subscribe, () => operations.snapshot(taskId), () => "[]");
  return JSON.parse(saved) as PendingCancellation[] | { error: string };
}

/**
 * Settles stop requests whose reply was lost (a dropped connection or a closed
 * window). It shows nothing: whenever the conversation changes it reads the
 * receipt of each one, and drops the record once the stop was accepted or the
 * run it targeted is over. Pressing stop again resends the same request.
 * `activeRunId` is undefined while the conversation is not loaded yet.
 */
export function CancelRecovery({ client, taskId, busy, activeRunId, revision = 0, onAccepted }: {
  client: CoreClient; taskId: TaskId; busy: boolean; activeRunId: string | null | undefined; revision?: number | string; onAccepted: () => void;
}) {
  const pending = usePendingCancellations(taskId);
  const records = Array.isArray(pending) ? pending : [];
  const operationsKey = records.map(record => record.operationId).join(",");
  const running = useRef(false), again = useRef(false), current = useRef({ client, taskId, activeRunId, onAccepted });
  current.current = { client, taskId, activeRunId, onAccepted };
  useEffect(() => {
    if (busy || !records.length) return;
    const origin = { client, taskId };
    const settle = async () => {
      if (running.current) { again.current = true; return; }
      running.current = true;
      try {
        for (const record of records) {
          try {
            const accepted = await operations.settle(client, record, current.current.activeRunId);
            const live = current.current;
            // A result for a conversation that is no longer shown is not reported.
            if (accepted && live.client === origin.client && live.taskId === origin.taskId) live.onAccepted();
          } catch {
            // Unreadable right now; the next conversation update checks again.
          }
        }
      } finally {
        running.current = false;
        if (again.current) { again.current = false; void settle(); }
      }
    };
    void settle();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- records are keyed by operationsKey
  }, [client, taskId, busy, revision, activeRunId, operationsKey]);
  return null;
}
