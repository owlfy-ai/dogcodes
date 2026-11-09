import { useId, useState } from "react";
import type { TaskId, TimelineItem } from "@dogcodes/protocol";
import { ActionIcon } from "./ReviewActions";
import { MarkdownMessage } from "./MarkdownMessage";
import { taskDisplayTitle } from "./displayNames";
import { useI18n } from "./i18n";
import "./forkOrigin.css";

interface ForkContext {
  sourceTaskId?: TaskId;
  environment?: "workspace" | "worktree";
  restoredTurnSnapshot?: boolean;
  contextOnly?: boolean;
  text: string;
}

const noteKey = (data: ForkContext) => data.environment === "worktree"
  ? data.restoredTurnSnapshot ? "activity.forkRestored" : data.contextOnly ? "activity.forkContextOnly" : "activity.forkLegacy"
  : "activity.forkSameDirectory";

/** Opens a forked conversation: where it came from, what it inherited, and that it can carry on. */
export function ForkOrigin({ item, alone, sourceTitle, onOpenTask }: {
  item: TimelineItem;
  /** Nothing has happened in the fork yet. */
  alone: boolean;
  sourceTitle?: (taskId: TaskId) => string | undefined;
  onOpenTask?: (taskId: TaskId) => void;
}) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const historyId = useId();
  let data: ForkContext, messages: { kind: string; text: string }[];
  try {
    data = JSON.parse(item.text) as ForkContext;
    messages = (JSON.parse(data.text) as { kind: string; text: string }[]).filter(m => m.kind === "turn.queued" || m.kind === "message.delta");
  } catch { return <p className="fork-origin-unavailable">{t("activity.forkUnavailable")}</p>; }
  const source = data.sourceTaskId;
  const title = source ? sourceTitle?.(source) : undefined;
  const prompts = messages.filter(m => m.kind === "turn.queued").length;
  return <section className="fork-origin" aria-label={t("activity.forkSummary")}>
    <header>
      <span className="fork-origin-icon"><ActionIcon name="fork"/></span>
      <div className="fork-origin-heading">
        <strong>{title === undefined ? t("activity.forkFromEarlier") : <>
          {t("activity.forkFrom")}{" "}
          {onOpenTask && source
            ? <button type="button" className="fork-origin-source" data-tooltip={t("activity.forkOpenSource")} onClick={() => onOpenTask(source)}>{taskDisplayTitle(title)}</button>
            : <span className="fork-origin-source">{taskDisplayTitle(title)}</span>}
        </>}</strong>
        <p>{t(noteKey(data))}</p>
      </div>
    </header>
    {messages.length > 0 && <>
      <button type="button" className="fork-origin-toggle" aria-expanded={open} aria-controls={historyId} onClick={() => setOpen(value => !value)}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m6 4 4 4-4 4"/></svg>
        {t(open ? "activity.forkHideContext" : "activity.forkShowContext", { count: prompts })}
      </button>
      <ol className="fork-origin-history" id={historyId} hidden={!open}>
        {messages.map((m, index) => <li key={index} className={m.kind === "turn.queued" ? "from-user" : "from-agent"}>
          <small>{m.kind === "turn.queued" ? t("activity.you") : t("activity.agent")}</small>
          {open && <MarkdownMessage text={m.text}/>}
        </li>)}
      </ol>
    </>}
    {alone && <footer>{t("activity.forkContinue")}</footer>}
  </section>;
}
