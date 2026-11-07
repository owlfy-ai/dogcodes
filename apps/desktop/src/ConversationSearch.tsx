import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import type { ConversationMatch, CoreClient, ProjectSummary, TaskId, TaskSummary } from "@dogcodes/protocol";
import { AgentIcon } from "./agentPresentation";
import { InlineError } from "./InlineError";
import { LoadingStatus } from "./LoadingStatus";
import { projectDisplayName, taskDisplayTitle } from "./displayNames";
import { translate as t } from "./i18n";
import "./conversationSearch.css";

const DEBOUNCE_MS = 200;
const REVEAL_TIMEOUT_MS = 2000;
const REVEAL_STEP_MS = 50;
const HIGHLIGHT_MS = 2200;

export interface ConversationSearchResult { task: TaskSummary; message?: ConversationMatch }

/** Titles answer at once and matter most, so they come first; the rest by the core's order. */
export function mergeSearchResults(tasks: TaskSummary[], query: string, messages: ConversationMatch[]): ConversationSearchResult[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const byTask = new Map(messages.map(message => [message.taskId as string, message]));
  const titled = tasks.filter(task => taskDisplayTitle(task.title).toLowerCase().includes(needle))
    .sort((a, b) => (b.updatedAt ?? "").localeCompare(a.updatedAt ?? ""));
  const seen = new Set<string>(titled.map(task => task.taskId));
  const known = new Map(tasks.map(task => [task.taskId as string, task]));
  return [
    ...titled.map(task => ({ task, message: byTask.get(task.taskId) })),
    ...messages.flatMap(message => {
      const task = known.get(message.taskId);
      return task && !seen.has(message.taskId) ? [{ task, message }] : [];
    }),
  ];
}

function Highlighted({ text, query }: { text: string; query: string }) {
  const needle = query.trim();
  if (!needle) return <>{text}</>;
  const pattern = new RegExp(`(${needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`, "iu");
  return <>{text.split(pattern).map((part, index) => index % 2 ? <mark key={index}>{part}</mark> : part)}</>;
}

/** Scrolls to a message once it's on the page and marks it briefly. A message in a turn's
 * collapsed steps isn't rendered until that turn is open. Returns a way to cancel. */
export function revealTimelineItem(itemId: string, turnId: string | null, done: () => void): () => void {
  const started = Date.now();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const find = (attribute: string, value: string) =>
    [...document.querySelectorAll<HTMLElement>(`[${attribute}]`)].find(element => element.getAttribute(attribute) === value);
  const step = () => {
    const element = find("data-timeline-item", itemId);
    if (element) {
      element.scrollIntoView?.({ block: "center" });
      element.classList.add("timeline-item-revealed");
      setTimeout(() => element.classList.remove("timeline-item-revealed"), HIGHLIGHT_MS);
      done();
      return;
    }
    const steps = turnId ? find("data-turn-process", turnId) : undefined;
    if (steps instanceof HTMLDetailsElement && !steps.open) steps.open = true;
    // The conversation is open either way; a message that never renders just isn't shown.
    if (Date.now() - started >= REVEAL_TIMEOUT_MS) { done(); return; }
    timer = setTimeout(step, REVEAL_STEP_MS);
  };
  step();
  return () => clearTimeout(timer);
}

export function ConversationSearch({ client, tasks, projects, shortcutKey, onOpen, onReveal, children }: {
  client: CoreClient; tasks: TaskSummary[]; projects: ProjectSummary[]; shortcutKey: string;
  onOpen: (taskId: TaskId, message?: { itemId: string; turnId: string | null }) => void;
  /** The field is about to be needed, so a collapsed sidebar has to open. */
  onReveal: () => void;
  children: ReactNode;
}) {
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<{ query: string; messages: ConversationMatch[] } | null>(null);
  const [failed, setFailed] = useState(false);
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const reveal = useRef(onReveal);
  reveal.current = onReveal;
  const listId = useId();
  const trimmed = query.trim();
  const searching = Boolean(trimmed);

  useEffect(() => {
    if (!trimmed || !client.searchConversations) { setFound(null); setFailed(false); return; }
    let current = true;
    const timer = setTimeout(() => {
      client.searchConversations!(trimmed).then(messages => {
        if (current) { setFound({ query: trimmed, messages }); setFailed(false); }
      }).catch(() => {
        if (current) { setFound({ query: trimmed, messages: [] }); setFailed(true); }
      });
    }, DEBOUNCE_MS);
    return () => { current = false; clearTimeout(timer); };
  }, [client, trimmed]);

  useEffect(() => {
    const focus = (event: KeyboardEvent) => {
      const modifier = /Mac|iPhone|iPad/.test(navigator.platform) ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (!modifier || !event.shiftKey || event.altKey || event.isComposing || event.key.toLowerCase() !== "f") return;
      event.preventDefault();
      // A shortcut must not reach behind a confirmation.
      if (document.querySelector('dialog[open], [role="dialog"][aria-modal="true"], [role="alertdialog"][aria-modal="true"]')) return;
      reveal.current();
      requestAnimationFrame(() => { input.current?.focus(); input.current?.select(); });
    };
    window.addEventListener("keydown", focus, true);
    return () => window.removeEventListener("keydown", focus, true);
  }, []);

  const messages = found?.query === trimmed ? found.messages : [];
  const results = useMemo(() => mergeSearchResults(tasks, trimmed, messages), [tasks, trimmed, messages]);
  const loading = searching && Boolean(client.searchConversations) && found?.query !== trimmed;
  useEffect(() => { setActive(0); }, [trimmed]);
  const highlighted = Math.min(active, Math.max(0, results.length - 1));
  const optionId = (index: number) => `${listId}-${index}`;
  const projectName = (task: TaskSummary) => {
    const project = projects.find(item => item.projectId === task.projectId);
    return project ? projectDisplayName(project.displayName) : "";
  };
  const open = (result: ConversationSearchResult) => onOpen(result.task.taskId, result.message && { itemId: result.message.itemId, turnId: result.message.turnId });

  return <>
    <div className="conversation-search" role="search">
      <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><circle cx="9" cy="9" r="5.5" /><path d="m13.2 13.2 3.8 3.8" /></svg>
      <input ref={input} type="text" role="combobox" value={query} maxLength={200} spellCheck={false} autoComplete="off"
        placeholder={t("conversationSearch.placeholder")} aria-label={t("conversationSearch.placeholder")}
        aria-keyshortcuts={`${shortcutKey}+Shift+F`} aria-expanded={searching} aria-controls={listId}
        aria-activedescendant={results.length ? optionId(highlighted) : undefined}
        onChange={event => setQuery(event.target.value)}
        onKeyDown={event => {
          if (event.nativeEvent.isComposing) return;
          if (event.key === "Escape" && query) { event.preventDefault(); event.stopPropagation(); setQuery(""); }
          else if (!results.length) return;
          else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const next = (highlighted + (event.key === "ArrowDown" ? 1 : -1) + results.length) % results.length;
            setActive(next);
            document.getElementById(optionId(next))?.scrollIntoView?.({ block: "nearest" });
          } else if (event.key === "Enter") { event.preventDefault(); open(results[highlighted]); }
        }} />
      {query
        ? <button type="button" aria-label={t("conversationSearch.clear")} data-tooltip={t("conversationSearch.clear")} onClick={() => { setQuery(""); input.current?.focus(); }}><svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true"><path d="m6 6 8 8M14 6l-8 8" /></svg></button>
        : <small aria-hidden="true">{/Mac|iPhone|iPad/.test(navigator.platform) ? "⌘⇧F" : "Ctrl+Shift+F"}</small>}
    </div>
    {/* Kept mounted, so what's expanded in the list survives a search. */}
    <div style={{ display: searching ? "none" : "contents" }}>{children}</div>
    {searching && <div className="conversation-list conversation-search-results">
      {failed && <InlineError>{t("conversationSearch.failed")}</InlineError>}
      <div id={listId} role="listbox" aria-label={t("conversationSearch.results")}>
        {results.map((result, index) => <button key={result.task.taskId} id={optionId(index)} type="button" role="option" tabIndex={-1}
          aria-selected={index === highlighted} className="conversation-search-result" onMouseEnter={() => setActive(index)} onClick={() => open(result)}>
          <AgentIcon providerId={(result.task.lastTurnExecutor ?? result.task.preferredExecutor).providerId} size="small" />
          <span className="conversation-search-text">
            <span className="conversation-search-title"><Highlighted text={taskDisplayTitle(result.task.title)} query={trimmed} /></span>
            {result.message && <span className="conversation-search-snippet">{result.message.fromUser && `${t("activity.you")}: `}<Highlighted text={result.message.snippet} query={trimmed} /></span>}
            <small>{projectName(result.task)}</small>
          </span>
        </button>)}
      </div>
      {loading && <LoadingStatus>{t("conversationSearch.searching")}</LoadingStatus>}
      {!loading && !results.length && !failed && <p className="history-empty" role="status">{t("conversationSearch.noResults")}</p>}
    </div>}
  </>;
}
