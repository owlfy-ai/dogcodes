import { LoadingStatus } from "./LoadingStatus";
import { TurnProcess } from "./TurnProcess";
import { TurnActions } from "./TurnActions";
import { DeferredDetails } from "./DeferredDetails";
import { AgentPending, type PendingAgent } from "./AgentPending";
import { ThinkingTicker } from "./ThinkingTicker";
import { ActivityIcon } from "./ActivityIcon";
import { ComputerUseActivity } from "./ComputerUseActivity";
import { useI18n } from "./i18n";
import { Fragment, memo, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  ArtifactMetadata,
  CoreClient,
  TaskId,
  TimelineItem,
} from "@dogcodes/protocol";
import { editPatch, reportedPatch } from "./conversationDiff";
import { MarkdownMessage, StreamingMarkdownMessage } from "./MarkdownMessage";
import { TurnFiles } from "./TurnFiles";
import { parseGitDiff } from "./diffView";
import { InlineFileEdit } from "./InlineFileEdit";
import { terminalCommand, terminalOutput, truncateTerminalOutput } from "./terminalPresentation";

export type Activity = {
  partial?: boolean;
  artifact?: ArtifactMetadata;
  type: string;
  name?: string;
  title?: string;
  status?: string;
  input?: unknown;
  output?: unknown;
  content?: unknown;
  category?: string;
  text?: string;
  parentId?: string;
  sessionId?: string;
  childSessionId?: string;
  childId?: string;
  details?: string;
  liveText?: string;
  /** Saved thinking text was cut at the store's bound. */
  textTruncated?: boolean;
  startedAt?: string;
  durationMs?: number;
  changes?: { path: string; diff?: string }[];
};
export function activityOf(item: TimelineItem): Activity | null {
  if (item.kind !== "activity") return null;
  try {
    const d = JSON.parse(item.text);
    return d && typeof d.type === "string" ? d : null;
  } catch {
    return null;
  }
}
const nativeId = (i: TimelineItem) =>
  i.itemId.replace(`tool:${i.taskId}:${i.turnId}:`, "");
export function isSubagent(a: Activity) {
  return (
    a.type === "subagent" ||
    /^(agent|task)$/i.test(a.name ?? "") ||
    /^Task:/.test(a.name ?? "")
  );
}
export type ActivityCategory = "subagent" | "edit" | "webSearch" | "read" | "search" | "terminal" | "fetch" | "tool";
export function categoryOf(a: Activity): ActivityCategory {
  if (isSubagent(a)) return "subagent";
  const name = (a.name ?? "").toLowerCase();
  if (
    ["edit", "delete", "move"].includes(a.category ?? "") ||
    /^(edit|write|multiedit|apply_patch|filechange|file_change|replace_in_file|write_file|write_to_file|replace_file_content)$/.test(
      name,
    )
  )
    return "edit";
  if (/^(websearch|web_search|search_web)$/.test(name)) return "webSearch";
  if (
    a.category === "read" ||
    /^(read|read_file|view_file|readfile)$/.test(name)
  )
    return "read";
  if (
    a.category === "search" ||
    /^(grep|glob|find_by_name|grep_search|search_files)$/.test(name)
  )
    return "search";
  if (
    a.category === "execute" ||
    /^(bash|terminal|commandexecution|command_execution|run_command|shell|exec_command)$/.test(
      name,
    )
  )
    return "terminal";
  if (
    a.category === "fetch" ||
    /^(webfetch|web_fetch|read_url_content)$/.test(name)
  )
    return "fetch";
  return "tool";
}
const printable = (v: unknown) =>
  typeof v === "string" ? v : v == null ? "" : JSON.stringify(v, null, 2);
function outputText(value: unknown): string {
  if (Array.isArray(value)) return value.map(outputText).join("\n");
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    if (typeof record.text === "string") return record.text;
    if (record.content != null) return outputText(record.content);
  }
  return printable(value);
}
function inputPath(a: Activity) {
  const i = a.input as Record<string, unknown> | undefined;
  return i && typeof i === "object"
    ? String(i.file_path ?? i.path ?? i.target_file ?? i.AbsolutePath ?? "")
    : "";
}
// Status ids map to activity.status.* keys.
const states: Record<string, string> = {
  pending: "pending",
  running: "running",
  started: "running",
  in_progress: "running",
  completed: "done",
  succeeded: "done",
  // Tool failures are routine agent recovery steps, not user-facing alerts.
  failed: "",
  interrupted: "interrupted",
  cancelled: "cancelled",
  unknown: "unknown",
};
function statusText(t: (key: string) => string, status: string | undefined): string {
  const key = states[status ?? ""] ?? "unknown";
  return key ? t(`activity.status.${key}`) : "";
}
const isRunning = (a: Activity, active: boolean) => active && ["running", "started", "in_progress"].includes(a.status ?? "");
function Thought({ a, active }: { a: Activity; active: boolean }) {
  const { t } = useI18n();
  const [, tick] = useState(0);
  const running = isRunning(a, active);
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(timer);
  }, [running]);
  const seconds =
    typeof a.durationMs === "number"
      ? Math.round(a.durationMs / 1000)
      : running && a.startedAt
        ? Math.max(0, Math.floor((Date.now() - Date.parse(a.startedAt)) / 1000))
        : null;
  const label = (
    <span>
      <ActivityIcon kind="think" />
      <span className="activity-text">
      {t(running
        ? "activity.thinking"
        : a.status === "interrupted"
          ? "activity.thinkingInterrupted"
          : "activity.thought")}
      {seconds !== null
        ? ` · ${t(running ? "activity.aboutSeconds" : "activity.lastedSeconds", { seconds })}`
        : ` · ${t("activity.timeNotRecorded")}`}
      </span>
    </span>
  );
  // A finished segment keeps its text: collapsed like a tool step, opened on demand.
  if (!running && a.text?.trim()) {
    return (
      <details className="conversation-activity thought-history">
        <summary className="activity-line">{label}</summary>
        <div className="activity-detail thought-detail">
          <MarkdownMessage text={a.text} />
          {a.textTruncated && <p className="subagent-notice">{t("activity.thoughtTruncated")}</p>}
        </div>
      </details>
    );
  }
  return (
    <div className={`thought-line${running ? " activity-running" : ""}`}>
      {label}
      {running && a.liveText && <ThinkingTicker text={a.liveText} />}
    </div>
  );
}
function FileEdits({ a, running, status }: { a: Activity; running: boolean; status: string }) {
  const input = a.input as Record<string, unknown> | undefined;
  const files = a.changes?.length
    ? a.changes
    : inputPath(a)
      ? [
          {
            path: inputPath(a),
            diff:
              input &&
              typeof input.old_string === "string" &&
              typeof input.new_string === "string"
                ? editPatch(inputPath(a), input.old_string, input.new_string)
                : undefined,
          },
        ]
      : [];
  return (
    <>
      {files.map((f, index) => {
        const parsed = f.diff
          ? parseGitDiff(
              new TextEncoder().encode(reportedPatch(f.path, f.diff)),
            )
          : null;
        return (
          <InlineFileEdit key={`${f.path}:${index}`} path={f.path}
            file={parsed?.kind === "structured" ? parsed.diff.files.find(file => (file.newPath || file.oldPath) === f.path) ?? parsed.diff.files[0] : undefined}
            rawPatch={parsed?.kind === "structured" ? undefined : f.diff}
            truncated={parsed?.kind === "structured" && parsed.diff.truncated}
            running={running} status={status} />
        );
      })}
    </>
  );
}
export function ActivityRow({
  item,
  a,
  active,
  onSubagent,
  client,
}: {
  item: TimelineItem;
  a: Activity;
  active: boolean;
  client?: CoreClient;
  onSubagent: (id: string) => void;
}) {
  const { t } = useI18n();
  // Native agents may finish a turn without sending a terminal tool update.
  // Preserve the uncertainty instead of showing an inactive tool as still waiting.
  const statusLabel = !active && ["pending", "running", "started", "in_progress"].includes(a.status ?? "")
    ? t("activity.resultUnconfirmed")
    : statusText(t, a.status);
  const running = isRunning(a, active);
  if (a.type === "status") return null;
  // A batch/progress notification with no display fields is not a tool row.
  if (a.type === "tool" && !a.name && !a.title && !a.status &&
      a.input == null && a.output == null && a.content == null &&
      !a.text && !a.changes?.length) return null;
  if (a.type === "thought") return <Thought a={a} active={active} />;
  if (a.type === "message")
    return (
      <article className="message assistant-message">
        <StreamingMarkdownMessage text={a.text ?? ""} streaming={active} />
      </article>
    );
  if (isSubagent(a))
    return (
      <button
        type="button"
        className={`activity-line${running ? " activity-running" : ""}`}
        onClick={() => onSubagent(item.itemId)}
      >
        <span className="activity-label"><ActivityIcon kind="subagent" /><span className="activity-text">{t("activity.category.subagent")}</span></span>
        <span className="activity-title activity-text">
          {a.title ||
            (a.input as { description?: string })?.description ||
            a.name ||
            t("activity.delegatedTask")}
        </span>
        <small className="activity-text">{statusLabel} ›</small>
      </button>
    );
  if (a.artifact && client)
    return (
      <>
        {categoryOf(a) !== "edit" && (
          <ActivityRow
            item={item}
            a={{ ...a, artifact: undefined }}
            active={active}
            onSubagent={onSubagent}
            client={client}
          />
        )}
        <TurnFiles
          taskId={item.taskId}
          metadata={a.artifact}
          client={client}
          operation
        />
      </>
    );
  if (categoryOf(a) === "edit" && (a.changes?.length || inputPath(a)))
    return <FileEdits a={a} running={running} status={statusLabel} />;
  const terminal = categoryOf(a) === "terminal";
  const command = terminal ? terminalCommand(a.input) : "";
  const preview = terminal ? truncateTerminalOutput(terminalOutput(a.output ?? a.content)) : null;
  const title = command ||
    inputPath(a) ||
    (typeof a.input === "string"
      ? a.input
      : ((a.input as { command?: string; pattern?: string })?.command ??
        (a.input as { pattern?: string })?.pattern)) ||
    a.name;
  return (
    <details className="conversation-activity">
      <summary
        className={`activity-line${running ? " activity-running" : ""}`}
      >
        <span className="activity-label">
          <ActivityIcon kind={categoryOf(a)} /><span className="activity-text">{t(`activity.category.${categoryOf(a)}`)}</span>
        </span>
        <span className="activity-title activity-text" data-tooltip={title}>
          {title}
        </span>
        {statusLabel && <small className="activity-text">{statusLabel}</small>}
      </summary>
      {terminal ? <div className="activity-detail terminal-activity-detail">
        {a.partial && <p className="subagent-notice">{t("activity.partial")}</p>}
        {command && <div className="terminal-activity-command"><span aria-hidden="true">$</span><pre>{command}</pre></div>}
        {preview?.text ? <pre className="terminal-activity-output">{preview.text}</pre> : <p className="terminal-activity-empty">{t("activity.noCommandOutput")}</p>}
        {preview?.truncated && <p className="terminal-activity-truncated">… {t("activity.outputTruncated")}</p>}
      </div> : <div className="activity-detail">
        {a.partial && <p className="subagent-notice">{t("activity.partial")}</p>}
        {a.input != null && <pre>{printable(a.input)}</pre>}
        {a.output != null || a.content != null ? (
          <pre>{outputText(a.output ?? a.content)}</pre>
        ) : (
          <p>{t("activity.noToolOutput")}</p>
        )}
      </div>}
    </details>
  );
}
const TimelineActivityRow = memo(function TimelineActivityRow(props: Omit<Parameters<typeof ActivityRow>[0], "a">) {
  const a = activityOf(props.item);
  return a ? <ActivityRow {...props} a={a} /> : null;
});

export function belongsTo(item: TimelineItem, parent: TimelineItem) {
  const a = activityOf(item);
  const p = activityOf(parent);
  return (
    !!a &&
    !!p &&
    item.turnId === parent.turnId &&
    item.itemId !== parent.itemId &&
    ((!!a.parentId && a.parentId === nativeId(parent)) ||
      (!!a.sessionId && a.sessionId === (p.childSessionId || p.childId)))
  );
}
export function ConversationContent({
  pendingAgent,
  items,
  client,
  activeTurnId,
  onSubagent,
  renderEntry,
  renderAgent,
  onReviewFile,
  onForked,
}: {
  items: TimelineItem[];
  pendingAgent?: PendingAgent;
  client: CoreClient;
  activeTurnId?: string;
  onSubagent: (id: string) => void;
  renderEntry: (i: TimelineItem) => React.ReactNode;
  renderAgent?: (providerId: string) => React.ReactNode;
  onReviewFile?: (path: string) => void;
  onForked?: (taskId: TaskId) => Promise<void> | void;
}) {
  const parents = items.filter((i) => {
    const a = activityOf(i);
    return a && isSubagent(a);
  });
  const visible = items.filter((i) => {
    const a = activityOf(i);
    const emptyMessage = i.kind === "assistant_message" ? !i.text.trim() : a?.type === "message" && !a.text?.trim();
    return i.kind !== "review_result" && !emptyMessage && !parents.some((p) => belongsTo(i, p));
  });
  const groups: TimelineItem[][] = [];
  const readOnly = (i: TimelineItem) => {
    const a = activityOf(i);
    return (
      !!a &&
      !a.artifact &&
      a.type === "tool" &&
      ["completed", "succeeded"].includes(a.status ?? "") &&
      ["read", "search", "webSearch", "fetch"].includes(categoryOf(a))
    );
  };
  for (const item of visible) {
    const previous = groups.at(-1);
    if (
      previous &&
      readOnly(item) &&
      readOnly(previous[0]) &&
      previous[0].turnId === item.turnId
    )
      previous.push(item);
    else groups.push([item]);
  }
  const firstResponse = new Map<string, string>();
  const turnProviders = new Map<string, string>();
  for (const item of visible) {
    if (!item.turnId) continue;
    if (item.providerId && item.kind !== "user_message") {
      turnProviders.set(item.turnId, item.providerId);
    }
    const a = activityOf(item);
    const response = item.kind === "assistant_message" || item.kind === "computer_use" || (a && a.type !== "status"
      && (a.type !== "tool" || a.name || a.title || a.status || a.input != null
        || a.output != null || a.content != null || a.text || a.changes?.length));
    if (response && !firstResponse.has(item.turnId)) firstResponse.set(item.turnId, item.itemId);
  }
  if (pendingAgent?.turnId && !turnProviders.has(pendingAgent.turnId)) {
    turnProviders.set(pendingAgent.turnId, pendingAgent.providerId);
  }
  const { t } = useI18n();
  const render = (item: TimelineItem) => {
    if (item.kind === "computer_use") return <ComputerUseActivity key={item.itemId} item={item} />;
    if (item.kind === "fork_context") {
      try { const data = JSON.parse(item.text); const messages = JSON.parse(data.text) as {kind:string;text:string}[];
        return <details className="turn-fork-history" key={item.itemId}><summary>{t("activity.forkSummary")}</summary><p>{t(data.environment === "worktree" ? (data.restoredTurnSnapshot ? "activity.forkRestored" : "activity.forkLegacy") : "activity.forkSameDirectory")}</p>{messages.filter(m => ["turn.queued","message.delta"].includes(m.kind)).map((m,index) => <article key={index}><small>{m.kind === "turn.queued" ? t("activity.you") : "Agent"}</small><MarkdownMessage text={m.text}/></article>)}</details>;
      } catch { return <p key={item.itemId}>{t("activity.forkUnavailable")}</p>; }
    }

    if (item.kind === "turn_files") {
      try {
        return (
          <TurnFiles
            key={item.itemId}
            taskId={item.taskId}
            providerId={item.turnId ? turnProviders.get(item.turnId) : undefined}
            onReviewFile={onReviewFile}
            metadata={JSON.parse(item.text)}
            client={client}
          />
        );
      } catch {
        return null;
      }
    }
    const a = activityOf(item);
    return a ? (
      <TimelineActivityRow
        key={item.itemId}
        item={item}
        active={!!activeTurnId && item.turnId === activeTurnId}
        onSubagent={onSubagent}
        client={client}
      />
    ) : (
      renderEntry(item)
    );
  };
  const renderGroup = (group: TimelineItem[], hideAgent = false) => {
    const first = group[0];
    const provider = first.turnId ? turnProviders.get(first.turnId) : undefined;
    const showAgent = !hideAgent && first.turnId && firstResponse.get(first.turnId) === first.itemId && provider && renderAgent;
    const content = group.length === 1 ? (
      render(group[0])
    ) : (
      <DeferredDetails key={group[0].itemId} className="conversation-activity" summary={
        <summary className="activity-line">
          <ActivityIcon kind="browse" /><span>{t("activity.browse")} ·{" "}
          {Object.entries(
            group.reduce<Record<string, number>>((counts, i) => {
              const category = categoryOf(activityOf(i)!);
              counts[category] = (counts[category] ?? 0) + 1;
              return counts;
            }, {}),
          )
            .map(([kind, count]) => t("activity.categoryCount", { category: t(`activity.category.${kind}`), count }))
            .join(" · ")}
          </span>
        </summary>}>
        {() => group.map(render)}
      </DeferredDetails>
    );
    return (
      <div className={showAgent ? "response-agent-anchor" : "conversation-entry"} key={first.itemId}>
        {showAgent && <span className="response-agent-badge">{renderAgent(provider)}</span>}
        {content}
      </div>
    );
  };
  const completions = new Map(visible.flatMap(item => item.turnId && item.turnCompletion
    ? [[item.turnId, item.turnCompletion] as const] : []));
  const renderedTurns = new Set<string>();
  const groupsByTurn = new Map<string, TimelineItem[][]>();
  for (const group of groups) {
    const turnId = group[0].turnId;
    if (!turnId) continue;
    const entries = groupsByTurn.get(turnId);
    if (entries) entries.push(group); else groupsByTurn.set(turnId, [group]);
  }
  const isAnswer = (item: TimelineItem) => item.kind === "assistant_message"
    ? Boolean(item.text.trim()) : activityOf(item)?.type === "message" && Boolean(activityOf(item)?.text?.trim());
  const awaitingResponse = pendingAgent && (!pendingAgent.turnId || !visible.some(item =>
    item.turnId === pendingAgent.turnId && (firstResponse.has(item.turnId) || item.kind.startsWith("tool."))));
  return <>{groups.map(group => {
    const turnId = group[0].turnId;
    const completion = turnId && completions.get(turnId);
    if (!turnId || !completion || turnId === activeTurnId) return renderGroup(group);
    if (renderedTurns.has(turnId)) return null;
    renderedTurns.add(turnId);
    const turnGroups = groupsByTurn.get(turnId)!;
    let finalIndex = -1;
    for (let index = turnGroups.length - 1; index >= 0; index--) {
      if (turnGroups[index].some(isAnswer)) { finalIndex = index; break; }
    }
    // Do not hide a turn that has no final answer to leave on screen.
    const process = turnGroups.slice(0, Math.max(0, finalIndex)).filter(entry =>
      entry[0].kind !== "user_message" && entry[0].kind !== "turn_files"
      && !(entry.length === 1 && activityOf(entry[0])?.type === "status"));
    const actions = <TurnActions key={`${turnId}-actions`} client={client} items={turnGroups.flat()} turnId={turnId} completedAt={completion.completedAt} startedAt={completion.startedAt} onReviewFile={onReviewFile} onForked={onForked}/>;
    if (!process.length) return <Fragment key={turnId}>{turnGroups.map(entry => renderGroup(entry))}{actions}</Fragment>;
    const provider = turnProviders.get(turnId);
    return <Fragment key={turnId}>
      {turnGroups.filter(entry => entry[0].kind === "user_message").map(entry => renderGroup(entry))}
      <TurnProcess key={`${turnId}-process`} startedAt={completion.startedAt} completedAt={completion.completedAt} agent={provider && renderAgent ? renderAgent(provider) : undefined}>
        {() => process.map(entry => renderGroup(entry, true))}
      </TurnProcess>
      {turnGroups.slice(finalIndex).filter(entry => entry[0].kind !== "turn_files" && entry[0].kind !== "user_message").map(entry => renderGroup(entry, true))}
      {turnGroups.filter(entry => entry[0].kind === "turn_files").map(entry => renderGroup(entry))}
      {actions}
    </Fragment>;
  })}{awaitingResponse && <AgentPending providerId={pendingAgent.providerId} renderAgent={renderAgent}/>}</>;

}
export function SubagentPanel({
  item,
  items,
  activeTurnId,
  onClose,
  client,
}: {
  item: TimelineItem;
  items: TimelineItem[];
  activeTurnId?: string;
  client?: CoreClient;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const a = activityOf(item)!;
  const children = items.filter((i) => belongsTo(i, item));
  const [drill, setDrill] = useState<string | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [history, setHistory] = useState<Activity[] | null>(null);
  const [error, setError] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    let valid = true;
    setHistory(null);
    setError(false);
    if (!a.details?.startsWith("history")) return;
    void invoke<Activity[]>("core_subagent_history", {
      taskId: item.taskId,
      itemId: item.itemId,
    }).then(
      (data) => {
        if (valid) setHistory(data);
      },
      () => {
        if (valid) setError(true);
      },
    );
    return () => {
      valid = false;
    };
  }, [item.taskId, item.itemId, a.details, retry]);
  const nested = drill
    ? items.find((i) => i.itemId === drill && i.itemId !== item.itemId)
    : undefined;
  if (nested)
    return (
      <SubagentPanel
        key={nested.itemId}
        item={nested}
        items={items}
        activeTurnId={activeTurnId}
        client={client}
        onClose={() => setDrill(null)}
      />
    );
  return (
    <aside className="subagent-panel" aria-label={t("activity.subagentProcess")}>
      <header>
        <ActivityIcon kind="subagent" />
        <b>{a.title || a.name || t("activity.category.subagent")}</b>
        <button type="button" aria-label={t("activity.closeSubagent")} onClick={onClose}>
          ✕
        </button>
      </header>
      <div className="subagent-content">
        <p className="subagent-notice">
          {statusText(t, history?.find((entry) => entry.type === "status")?.status ?? a.status)}
        </p>
        {error ? (
          <p role="alert">
            {t("activity.subagentLoadFailed")}{" "}
            <button type="button" onClick={() => setRetry((n) => n + 1)}>
              {t("common.retry")}
            </button>
          </p>
        ) : a.details?.startsWith("history") && history === null ? (
          <LoadingStatus layout="panel">{t("activity.loadingRecords")}</LoadingStatus>
        ) : null}
        {children.length === 0 && !a.details?.startsWith("history") && (
          <p className="subagent-notice">
            {t(a.details === "live_only" ? "activity.liveOnly"
              : a.details === "result_only" ? "activity.resultOnly"
              : a.details === "available" ? "activity.noRecords"
              : "activity.detailsUnsupported")}
          </p>
        )}
        {history !== null && (
          <p className="subagent-notice">
            {t(a.details === "history_partial"
              ? "activity.partialRecords"
              : "activity.records")}{" "}
            <button type="button" onClick={() => setRetry((n) => n + 1)}>
              {t("activity.refresh")}
            </button>
          </p>
        )}
        {children.map((i) => (
          <ActivityRow
            key={i.itemId}
            item={i}
            a={activityOf(i)!}
            active={!!activeTurnId && i.turnId === activeTurnId}
            onSubagent={setDrill}
            client={client}
          />
        ))}
        {history?.map((data, index) => (
          <ActivityRow
            key={index}
            item={item}
            a={data}
            active={false}
            onSubagent={() => setUnsupported(true)}
          />
        ))}
        {history?.length === 0 && (
          <p className="subagent-notice">{t("activity.noRecords")}</p>
        )}
        {unsupported && (
          <p className="subagent-notice">
            {t("activity.subagentDetailsUnsupported")}
          </p>
        )}
        {a.details === "result_only" && a.input != null && <MarkdownMessage text={outputText(a.input)} />}
        {a.output != null && <MarkdownMessage text={outputText(a.output)} />}
      </div>
    </aside>
  );
}
