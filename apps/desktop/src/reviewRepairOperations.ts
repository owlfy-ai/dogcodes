import { ulid } from "ulid";
import type { CoreClient, ExecutorRef, ReviewFixRef, ReviewRepairCall, ReviewRepairEntry, TaskId } from "@dogcodes/protocol";
import { withClientOperationLock as serialized } from "./clientOperationLock";
import { translate as t } from "./i18n";

// The service freezes and drives the repair; this client only submits the
// request and reads the shared record. Nothing is kept in local storage, so a
// repair started elsewhere (another window or device) is visible here too.
const listeners = new Set<() => void>();
const emit = () => listeners.forEach(listener => listener());
const key = (task: string, run: string) => `review-repair:${task}:${run}`;

function api(client: CoreClient) {
  if (!client.reviewRepair) throw new Error(t("repairOps.unsupported"));
  return (call: ReviewRepairCall) => client.reviewRepair!(call);
}
function get(client: CoreClient, taskId: TaskId, runId: string): Promise<ReviewRepairEntry | null> {
  return api(client)({ action: "get", taskId, runId });
}
async function start(client: CoreClient, taskId: TaskId, reference: ReviewFixRef, target?: ExecutorRef): Promise<ReviewRepairEntry> {
  const call = api(client);
  const findingIndices = [...new Set(reference.findingIndices)].sort((a, b) => a - b);
  if (!reference.inputSha256 || !findingIndices.length) throw new Error(t("repairOps.invalidSelection"));
  const operationId = `cmd_${ulid()}`;
  return serialized(key(taskId, reference.runId), async () => {
    try {
      const entry = await call({ action: "start", operationId, taskId, reference: { ...reference, findingIndices }, target: target ?? null });
      if (!entry) throw new Error(t("repair.unconfirmed"));
      return entry;
    } catch (cause) {
      // The service keeps working after a lost reply; trust only its record of this exact request.
      const latest = await get(client, taskId, reference.runId).catch(() => null);
      if (latest?.intent.operationId === operationId && latest.state === "submitted") return latest;
      throw cause;
    } finally { emit(); }
  });
}
function original(action: "resume" | "abandon") {
  return (client: CoreClient, entry: ReviewRepairEntry): Promise<ReviewRepairEntry> => {
    const call = api(client), { taskId, reference, operationId } = entry.intent;
    return serialized(key(taskId, reference.runId), async () => {
      try {
        const latest = await call({ action, taskId, runId: reference.runId, operationId });
        if (!latest) throw new Error(t("repairOps.missing"));
        return latest;
      } finally { emit(); }
    });
  };
}
export const reviewRepairOperations = {
  start, get, resume: original("resume"), abandon: original("abandon"),
  subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
};
