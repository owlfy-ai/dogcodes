import type { CoreClient } from "@dogcodes/protocol";
import { translate as t } from "./i18n";

const taskId = (value: unknown): value is string => typeof value === "string" && /^tsk_[0-9A-HJKMNP-TV-Z]{26}$/.test(value);
interface ObjectRecord {
  taskId?: unknown; turnId?: unknown; newTaskId?: unknown; runId?: unknown; operationId?: unknown; method?: unknown;
  request?: { taskId?: unknown; operationId?: unknown };
  task?: { taskId?: unknown; activePrimaryRun?: { runId?: unknown } };
  reference?: { runId?: unknown };
  query?: { taskId?: unknown; turnId?: unknown; requestedAtSequence?: unknown; approvalId?: unknown; operationId?: unknown };
}
/** Only identify records whose key agrees with their payload. Corrupt records stay untouched. */
function owner(key: string, raw: string): string | null {
  const known = ["dogcodes:pending-git:", "dogcodes:pending-review-mutation:", "dogcodes:pending-fork:", "dogcodes:pending-review-start:", "dogcodes:pending-review-repair:", "dogcodes:pending-cancel:", "dogcodes:pending-approval:", "dogcodes.project-run.pending."];
  const prefix = known.find(prefix => key.startsWith(prefix));
  if (!prefix) return null;
  const value = JSON.parse(raw) as ObjectRecord;
  if (!value || typeof value !== "object") throw new Error(t("deletedRecords.corrupt"));
  let task: unknown, expected: string | undefined;
  if (prefix === "dogcodes:pending-git:" || prefix === "dogcodes:pending-review-mutation:") {
    task = value.request?.taskId;
    if (typeof value.request?.operationId === "string") expected = prefix + value.request.operationId;
  } else if (prefix === "dogcodes:pending-fork:") {
    task = value.taskId;
    if (typeof value.turnId === "string" && typeof value.newTaskId === "string") expected = `${prefix}${task}:${value.turnId}`;
  } else if (prefix === "dogcodes:pending-review-start:") {
    task = value.taskId;
    if (typeof value.runId === "string" && key.startsWith(`${prefix}${task}:`)) expected = key;
  } else if (prefix === "dogcodes:pending-review-repair:") {
    task = value.task?.taskId;
    if (typeof value.reference?.runId === "string" && typeof value.operationId === "string") expected = `${prefix}${task}:${value.reference.runId}`;
  } else if (prefix === "dogcodes:pending-cancel:") {
    task = value.task?.taskId;
    if (typeof value.task?.activePrimaryRun?.runId === "string" && typeof value.operationId === "string") expected = `${prefix}${task}:${value.task.activePrimaryRun.runId}`;
  } else if (prefix === "dogcodes:pending-approval:") {
    const query = value.query; task = query?.taskId;
    if (typeof query?.turnId === "string" && Number.isSafeInteger(query.requestedAtSequence) && typeof query.approvalId === "string" && typeof query.operationId === "string")
      expected = `${prefix}${task}:${query.turnId}:${query.requestedAtSequence}:${encodeURIComponent(query.approvalId)}:${query.operationId}`;
  } else {
    task = value.request?.taskId;
    if (typeof value.request?.operationId === "string" && typeof value.method === "string" && ["core_project_runs_analyze", "core_project_run_edit"].includes(value.method)) expected = `${prefix}${task}.${value.request.operationId}`;
  }
  if (!taskId(task) || key !== expected) throw new Error(t("deletedRecords.owner"));
  return task;
}

/** Read-only deletion proof is required; bootstrap absence and RPC errors never authorize removal. */
export async function retireDeletedOperationRecords(client: CoreClient, current: () => boolean = () => true) {
  const result = { retired: 0, issues: [] as string[] };
  if (!client.taskDeletion) return result;
  const grouped = new Map<string, { key: string; raw: string }[]>();
  // Snapshot keys first: removal or another window must not shift iteration.
  const keys = Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index)).filter((key): key is string => key !== null);
  for (const key of keys) {
    try {
      const raw = localStorage.getItem(key); if (raw === null) continue;
      const task = owner(key, raw); if (!task) continue;
      const records = grouped.get(task) ?? []; records.push({ key, raw }); grouped.set(task, records);
    } catch { result.issues.push(t("deletedRecords.unreadable")); }
  }
  for (const [task, records] of grouped) {
    if (!current()) break;
    try {
      const proof = await client.taskDeletion(task);
      if (!current()) break;
      if (!proof) continue;
      if (proof.taskId !== task || !Number.isSafeInteger(proof.deletionSequence) || proof.deletionSequence < 1) throw new Error(t("deletedRecords.proof"));
      for (const record of records) {
        if (!current()) break;
        try {
          if (localStorage.getItem(record.key) !== record.raw) continue;
          localStorage.removeItem(record.key); result.retired++;
          window.dispatchEvent(new StorageEvent("storage", { key: record.key, oldValue: record.raw, newValue: null, storageArea: localStorage }));
        } catch { result.issues.push(t("deletedRecords.notClosed")); }
      }
    } catch { result.issues.push(t("deletedRecords.verifyFailed")); }
  }
  return result;
}

export function watchDeletedOperationRecords(client: CoreClient) {
  if (!client.taskDeletion) return () => {};
  let active = true, running = false, dirty = false;
  let timer: ReturnType<typeof setTimeout>;
  const refresh = () => {
    if (!active) return;
    if (running) { dirty = true; return; }
    running = true;
    void retireDeletedOperationRecords(client, () => active).catch(() => { /* Keep records if storage is unavailable. */ }).finally(() => {
      running = false;
      if (active && dirty) { dirty = false; clearTimeout(timer); timer = setTimeout(refresh, 250); }
    });
  };
  const schedule = () => { clearTimeout(timer); timer = setTimeout(refresh, 250); };
  const fallback = setInterval(refresh, 30000);
  window.addEventListener("focus", schedule); window.addEventListener("storage", schedule); window.addEventListener("dogcodes:task-deleted", schedule);
  refresh();
  return () => { active = false; clearTimeout(timer); clearInterval(fallback); window.removeEventListener("focus", schedule); window.removeEventListener("storage", schedule); window.removeEventListener("dogcodes:task-deleted", schedule); };
}
