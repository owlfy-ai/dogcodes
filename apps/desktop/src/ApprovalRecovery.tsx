import { useEffect, useRef, useSyncExternalStore } from "react";
import type { CoreClient, TaskId } from "@dogcodes/protocol";
import { approvalOperations as operations, type PendingApprovalResponse } from "./approvalOperations";

export function usePendingApprovalResponses(taskId: TaskId | null) {
  const saved = useSyncExternalStore(operations.subscribe, () => operations.snapshot(taskId), () => "[]");
  return JSON.parse(saved) as PendingApprovalResponse[] | { error: string };
}

/**
 * Settles approval answers whose outcome was not confirmed when they were sent
 * (a lost connection or a closed window). It shows nothing: it re-reads the
 * receipt of each pending answer whenever the conversation changes, and drops
 * the local record once the backend accepted or superseded it. `revision`
 * changes with the conversation version and its pending approvals. An answer whose
 * receipt is still missing stays pending; answering the card again resends the
 * same request, and a different answer voids the original first.
 */
export function ApprovalRecovery({ client, taskId, busy, revision = 0, onAccepted }: { client: CoreClient; taskId: TaskId; busy: boolean; revision?: number | string; onAccepted: (record: PendingApprovalResponse) => void }) {
  const pending = usePendingApprovalResponses(taskId);
  const records = Array.isArray(pending) ? pending : [];
  const operationsKey = records.map(record => record.query.operationId).join(",");
  const running = useRef(false), again = useRef(false), current = useRef({ client, taskId, onAccepted });
  current.current = { client, taskId, onAccepted };
  useEffect(() => {
    if (busy || !records.length) return;
    const origin = { client, taskId };
    const settle = async () => {
      if (running.current) { again.current = true; return; }
      running.current = true;
      try {
        for (const record of records) {
          try {
            const result = await operations.check(client, record);
            const live = current.current;
            // A result for a conversation that is no longer shown is not reported.
            if (result.kind === "missing" || live.client !== origin.client || live.taskId !== origin.taskId) continue;
            live.onAccepted(record);
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
  }, [client, taskId, busy, revision, operationsKey]);
  return null;
}
