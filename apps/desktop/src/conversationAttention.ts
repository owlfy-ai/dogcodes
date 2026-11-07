import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CoreClient, EventEnvelope, TaskId, TaskSummary } from "@dogcodes/protocol";
import { taskDisplayTitle } from "./displayNames";
import { translate as t } from "./i18n";

export type WaitingKind = "approval" | "question";
export type UnreadKind = "failed" | "completed";
export type AttentionKind = WaitingKind | UnreadKind;
export type ConversationAttention = Record<string, AttentionKind>;

const UNREAD_KEY = "dogcodes.attention.unread.v1";
const NOTIFICATIONS_KEY = "dogcodes.notifications.v1";
const PRIORITY: AttentionKind[] = ["approval", "question", "failed", "completed"];
const NOTIFICATION_BODY: Record<AttentionKind, string> = {
  approval: "attention.notifyApproval", question: "attention.notifyQuestion",
  failed: "attention.notifyFailed", completed: "attention.notifyCompleted",
};

export const attentionLabel = (kind: AttentionKind) => t(`attention.${kind}`);

/** The one to show for a group of conversations, such as a collapsed project. */
export function strongestAttention(kinds: (AttentionKind | undefined)[]): AttentionKind | undefined {
  return PRIORITY.find(kind => kinds.includes(kind));
}

function readUnread(): Record<string, UnreadKind> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(UNREAD_KEY) ?? "{}");
    return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, UnreadKind> : {};
  } catch { return {}; }
}
function writeUnread(value: Record<string, UnreadKind>) {
  try { localStorage.setItem(UNREAD_KEY, JSON.stringify(value)); } catch { /* Storage can be disabled. */ }
}
function without<T>(record: Record<string, T>, key: string) {
  if (!(key in record)) return record;
  const next = { ...record };
  delete next[key];
  return next;
}

/** What each conversation needs from the person, from one source for the sidebar, the
 * Dock badge and system notifications. What's waiting comes from the core, so it survives
 * a restart; a finished turn nobody has looked at yet is remembered here. */
export function useConversationAttention({ client, tasks, viewingTaskId, onOpen }: { client: CoreClient; tasks: TaskSummary[] | undefined; viewingTaskId: TaskId | null; onOpen: (taskId: TaskId) => void }) {
  const [waiting, setWaiting] = useState<Record<string, WaitingKind>>({});
  const [unread, setUnread] = useState(readUnread);
  const [notifications, setNotificationsState] = useState(() => {
    try { return localStorage.getItem(NOTIFICATIONS_KEY) !== "false"; } catch { return true; }
  });
  const latest = useRef({ tasks, viewingTaskId, notifications, onOpen });
  latest.current = { tasks, viewingTaskId, notifications, onOpen };
  const requests = useRef(new Map<string, number>());

  const setNotifications = useCallback((enabled: boolean) => {
    setNotificationsState(enabled);
    try { localStorage.setItem(NOTIFICATIONS_KEY, String(enabled)); } catch { /* Storage can be disabled. */ }
  }, []);

  const refreshWaiting = useCallback(async (taskId: string) => {
    const request = (requests.current.get(taskId) ?? 0) + 1;
    requests.current.set(taskId, request);
    try {
      const pending = await client.pendingApprovals(taskId as TaskId);
      if (requests.current.get(taskId) !== request) return;
      setWaiting(current => pending.length
        ? { ...current, [taskId]: pending.some(item => item.question) ? "question" : "approval" }
        : without(current, taskId));
    } catch { /* The next event or run change asks again. */ }
  }, [client]);

  const withdraw = useCallback((taskId: string) => {
    if (isTauri()) void invoke("withdraw_attention", { taskId }).catch(() => {});
  }, []);
  const seeing = (taskId: string) => latest.current.viewingTaskId === taskId && document.hasFocus();
  const notify = (taskId: string, kind: AttentionKind) => {
    if (!latest.current.notifications || !isTauri()) return;
    const task = latest.current.tasks?.find(item => item.taskId === taskId);
    void invoke("notify_attention", { taskId, title: task ? taskDisplayTitle(task.title) : "dog.codes", body: t(NOTIFICATION_BODY[kind]) }).catch(() => {});
  };
  const markUnread = (taskId: string, kind: UnreadKind) => setUnread(current => {
    const next = { ...current, [taskId]: kind };
    writeUnread(next);
    return next;
  });
  const clearUnread = useCallback((taskId: string) => setUnread(current => {
    const next = without(current, taskId);
    if (next !== current) writeUnread(next);
    return next;
  }), []);

  const handleEvent = useRef((_: EventEnvelope) => {});
  const onEvent = useCallback((event: EventEnvelope) => handleEvent.current(event), []);
  handleEvent.current = (event: EventEnvelope) => {
    if (event.stream.streamType !== "task") return;
    const taskId = event.stream.streamId;
    if (event.eventType === "approval.requested") {
      void refreshWaiting(taskId);
      const { question } = event.payload as { question?: unknown };
      if (!seeing(taskId)) notify(taskId, question ? "question" : "approval");
    } else if (event.eventType === "approval.resolved") {
      void refreshWaiting(taskId);
      withdraw(taskId);
    } else if (event.eventType === "turn.queued") {
      clearUnread(taskId);
    } else if (event.eventType === "turn.completed" || event.eventType === "turn.failed") {
      if (seeing(taskId)) return;
      const kind = event.eventType === "turn.failed" ? "failed" : "completed";
      markUnread(taskId, kind);
      notify(taskId, kind);
    }
  };

  // A click on a notification. It can come long after, or launch the app, so this waits for
  // the conversations to load and the one it was about may be gone.
  const clicked = useRef<string | null>(null);
  const openClicked = useCallback(() => {
    const { tasks: known, onOpen: open } = latest.current;
    if (!clicked.current || !known) return;
    const task = known.find(item => item.taskId === clicked.current);
    clicked.current = null;
    if (task) open(task.taskId);
  }, []);
  useEffect(openClicked, [tasks, openClicked]);
  useEffect(() => {
    if (!isTauri()) return;
    const receive = (taskId: string | null | undefined) => {
      if (!taskId) return;
      clicked.current = taskId;
      openClicked();
    };
    let stopped = false, stop: (() => void) | undefined;
    void listen<string>("attention-open", ({ payload }) => receive(payload)).then(unlisten => {
      if (stopped) { unlisten(); return; }
      stop = unlisten;
      return invoke<string | null>("take_attention_click").then(receive);
    }).catch(() => {});
    return () => { stopped = true; stop?.(); };
  }, [openClicked]);

  // Approvals raised before this window opened, or while its event stream was reconnecting.
  const activeTasks = (tasks ?? []).filter(task => task.activePrimaryRun).map(task => task.taskId).join();
  useEffect(() => {
    for (const taskId of activeTasks ? activeTasks.split(",") : []) void refreshWaiting(taskId);
  }, [activeTasks, refreshWaiting]);

  useEffect(() => {
    const clear = () => {
      if (!viewingTaskId || !document.hasFocus()) return;
      clearUnread(viewingTaskId);
      withdraw(viewingTaskId);
    };
    clear();
    window.addEventListener("focus", clear);
    return () => window.removeEventListener("focus", clear);
  }, [viewingTaskId, clearUnread, withdraw]);

  // A deleted conversation can't be looked at, so its mark would never clear.
  useEffect(() => {
    if (!tasks) return;
    const live = new Set<string>(tasks.map(task => task.taskId));
    setUnread(current => {
      const kept = Object.fromEntries(Object.entries(current).filter(([taskId]) => live.has(taskId)));
      if (Object.keys(kept).length === Object.keys(current).length) return current;
      writeUnread(kept);
      return kept;
    });
  }, [tasks]);

  const attention = useMemo(() => {
    const result: ConversationAttention = {};
    for (const task of tasks ?? []) {
      const kind = (task.activePrimaryRun ? waiting[task.taskId] : undefined) ?? unread[task.taskId];
      if (kind) result[task.taskId] = kind;
    }
    return result;
  }, [tasks, waiting, unread]);

  // Finished work isn't counted: it can wait, and a badge that never clears gets ignored.
  const needingAction = notifications ? Object.values(attention).filter(kind => kind !== "completed").length : 0;
  useEffect(() => {
    if (isTauri()) void invoke("set_attention_badge", { count: needingAction }).catch(() => {});
  }, [needingAction]);

  return { attention, onEvent, notifications, setNotifications };
}
