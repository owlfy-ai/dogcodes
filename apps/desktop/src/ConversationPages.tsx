import { createContext, useRef, type ReactNode } from "react";
import { ConversationScroll } from "./ConversationScroll";

export const ConversationPageVisible = createContext(true);

/** Keep visited histories mounted so Markdown, diffs and disclosure state survive navigation. */
export function ConversationPages({ taskId, revision, status, latestUserMessageId, transient = false, children }: { taskId: string | null; revision: unknown; status?: string; latestUserMessageId?: string; transient?: boolean; children: ReactNode }) {
  const pages = useRef(new Map<string, { revision: unknown; status?: string; latestUserMessageId?: string; content: ReactNode }>());
  if (taskId && !transient && (pages.current.get(taskId)?.revision !== revision || pages.current.get(taskId)?.status !== status || pages.current.get(taskId)?.latestUserMessageId !== latestUserMessageId)) {
    pages.current.set(taskId, { revision, status, latestUserMessageId, content: children });
  }
  return <>{[...pages.current].map(([id, page]) => <div key={id} hidden={transient || id !== taskId} style={{ display: !transient && id === taskId ? "contents" : "none" }}>
    <ConversationPageVisible.Provider value={!transient && id === taskId}><ConversationScroll taskId={id} active={!transient && id === taskId} latestUserMessageId={page.latestUserMessageId}>{page.content}</ConversationScroll></ConversationPageVisible.Provider>
  </div>)}{transient && taskId && <ConversationScroll taskId={taskId} latestUserMessageId={latestUserMessageId}>{children}</ConversationScroll>}</>;
}
