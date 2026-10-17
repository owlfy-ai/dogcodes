import type { TaskId } from "@dogcodes/protocol";
import { useCallback, useMemo, useRef, useState } from "react";

export type ConversationVisits<T extends string = string> = { entries: T[]; index: number };
export function visitConversation<T extends string>(history: ConversationVisits<T>, taskId: T): ConversationVisits<T> {
  if (history.entries[history.index] === taskId) return history;
  const entries = [...history.entries.slice(0, history.index + 1), taskId].slice(-100);
  return { entries, index: entries.length - 1 };
}

export function conversationNavigationIndex<T extends string>(history: ConversationVisits<T>, current: T | null, direction: -1 | 1, available: Set<T>): number | null {
  // An unsent draft is outside the visit history; Back returns to the last conversation.
  if (current === null && direction === 1) return null;
  const start = current === null ? history.index : history.index + direction;
  for (let index = start; index >= 0 && index < history.entries.length; index += direction) {
    const taskId = history.entries[index];
    if (taskId !== current && available.has(taskId)) return index;
  }
  return null;
}

export function useConversationNavigation(current: TaskId | null, taskIds: TaskId[]) {
  const history = useRef<ConversationVisits<TaskId>>({ entries: [], index: -1 });
  const [, refresh] = useState(0);
  const record = useCallback((taskId: TaskId) => {
    const next = visitConversation(history.current, taskId);
    if (next !== history.current) { history.current = next; refresh(value => value + 1); }
  }, []);
  const available = useMemo(() => new Set(taskIds), [taskIds]);
  const target = (direction: -1 | 1) => conversationNavigationIndex(history.current, current, direction, available);
  return {
    record,
    canBack: target(-1) !== null,
    canForward: target(1) !== null,
    move(direction: -1 | 1) {
      const index = target(direction);
      if (index === null) return null;
      history.current = { ...history.current, index };
      refresh(value => value + 1);
      return history.current.entries[index];
    },
  };
}
