import { LoadingStatus } from "./LoadingStatus";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { userFacingError } from "./userFacingError";
import { SentAttachments } from "./attachments";
import { TurnFiles } from "./TurnFiles";
import { ArtifactBrowser } from "./ArtifactBrowser";
import { useEffect, useRef, useState } from "react";
import type { ArtifactMetadata, CoreClient, HistoryPage, HistoryQuery, TaskId } from "@dogcodes/protocol";
import { translate as t } from "./i18n";
export function HistoryBrowser({client, taskId, onBack}: {client: CoreClient; taskId: TaskId; onBack: () => void}) {
  const [artifactsOpen, setArtifactsOpen] = useState(false);
  const [text, setText] = useState("");
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<HistoryQuery["kind"]>("turns");
  const [page, setPage] = useState<HistoryPage | null>(null);
  const [items, setItems] = useState<HistoryPage["items"]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const searchInput = useRef<HTMLInputElement>(null);
  async function load(more = false) {
    const id = ++generation.current;
    setBusy(true); setError("");
    try {
      if (!client.queryHistory) throw new Error(t("historyBrowser.desktopOnly"));
      const result = await client.queryHistory({taskId, kind, text: query, afterSequence: more ? page?.nextCursor ?? 0 : 0, throughSequence: more ? page?.throughSequence : null, limit: 30});
      if (id !== generation.current) return;
      setPage(result); setItems(old => more ? [...old, ...result.items] : result.items);
    } catch (e) { if (id === generation.current) setError(userFacingError(e, t("historyBrowser.loadFailed"))); }
    finally { if (id === generation.current) setBusy(false); }
  }
  useEffect(() => { setItems([]); setPage(null); void load(); return () => { generation.current++; }; }, [client, taskId, kind, query]);
  return <section className="history-browser">
    <button type="button" onClick={onBack}>← {t("recovery.back")}</button>
    <h2>{t("historyBrowser.title")}</h2>
    <button type="button" onClick={() => setArtifactsOpen(true)}>{t("historyBrowser.viewArtifacts")}</button>
    {artifactsOpen && <ArtifactBrowser key={taskId} client={client} taskId={taskId} onClose={() => setArtifactsOpen(false)} />}
    <form onSubmit={e => { e.preventDefault(); setKind(text.trim() ? "search" : "turns"); setQuery(text.trim()); }}>
      <input ref={searchInput} aria-label={t("historyBrowser.searchLabel")} placeholder={t("historyBrowser.searchPlaceholder")} value={text} onChange={e => setText(e.target.value)} />
      <button type="submit">{t("historyBrowser.search")}</button>
    </form>
    <nav aria-label={t("historyBrowser.kinds")}>{(["turns", "decisions", "changes"] as const).map(value => <button key={value} type="button" aria-pressed={kind === value} onClick={() => {setKind(value); setQuery(""); setText("");}}>{t(`historyBrowser.kind.${value}`)}</button>)}</nav>
    <p>{t("historyBrowser.description")}</p>
    {items.map(item => <HistoryRecord key={`${taskId}:${item.sequence}`} item={item} client={client} taskId={taskId} />)}
    {!busy && !items.length && !error && <ReviewEmptyState kind={query ? "search" : "history"} compact
      title={t(page?.nextCursor != null ? "historyBrowser.emptyPage" : query ? "historyBrowser.noResults" : kind === "decisions" ? "historyBrowser.noDecisions" : kind === "changes" ? "historyBrowser.noChanges" : "historyBrowser.noTurns")}
      description={t(page?.nextCursor != null ? "historyBrowser.moreToSearch" : query ? "historyBrowser.tryOther" : "historyBrowser.emptyHint")}
      action={<>{page?.nextCursor != null && <button type="button" className="review-empty-action" onClick={() => void load(true)}>{t("historyBrowser.continue")}</button>}{query ? <button type="button" className={`review-empty-action${page?.nextCursor != null ? " review-empty-action--secondary" : ""}`} onClick={() => { setText(""); setQuery(""); setKind("turns"); searchInput.current?.focus(); }}>{t("historyBrowser.clear")}</button> : page?.nextCursor == null && <button type="button" className="review-empty-action" onClick={onBack}>{t("recovery.back")}</button>}</>} />}
    {error && <p role="alert">{error} <button type="button" onClick={() => void load(items.length > 0)}>{t("common.retry")}</button></p>}
    {busy && <LoadingStatus layout={items.length ? "inline" : "panel"}>{t("historyBrowser.querying")}</LoadingStatus>}
    {page?.nextCursor != null && items.length > 0 && <button disabled={busy} type="button" onClick={() => void load(true)}>{t("historyBrowser.nextPage")}</button>}
  </section>;
}

function HistoryRecord({ item, client, taskId }: { item: HistoryPage["items"][number]; client: CoreClient; taskId: TaskId }) {
  const [expanded, setExpanded] = useState(false);
  const labels: Record<string, string> = {user_message: t("activity.you"), assistant_message: "Agent", system: t("historyBrowser.record.system"), activity: t("activity.records"), turn_files: t("historyBrowser.kind.changes"), "decision.recorded": t("historyBrowser.kind.decisions"), "workspace.turn_changes_captured": t("historyBrowser.kind.changes"), "workspace.turn_changes_failed": t("historyBrowser.record.changesFailed")};
  let data: Record<string, unknown> | null = null;
  if (!["user_message", "assistant_message", "system"].includes(item.kind)) {
    try { const value: unknown = JSON.parse(item.text); if (value && typeof value === "object" && !Array.isArray(value)) data = value as Record<string, unknown>; } catch { /* Plain text records remain literal. */ }
  }
  const candidate = data?.artifact ?? (item.kind === "turn_files" ? data : null);
  const metadata = candidate && typeof candidate === "object" && "artifactId" in candidate && "payloadSha256" in candidate && "byteLength" in candidate && "mediaType" in candidate && typeof candidate.mediaType === "string" && /\.(?:turn|operation)-changes\.v[12]\+json$/.test(candidate.mediaType) ? candidate as ArtifactMetadata : null;
  const summary = data ? [data.statement, data.message, data.summary, data.title, data.name].find(value => typeof value === "string") as string | undefined : item.text;
  return <article>
    <small>{labels[item.kind] ?? t("nav.history")} · {new Date(item.occurredAt).toLocaleString()}</small>
    {summary && <pre>{summary}</pre>}
    {item.kind === "user_message" && <SentAttachments items={item.attachments ?? []} />}
    {metadata ? <div><button type="button" onClick={() => setExpanded(value => !value)}>{t(expanded ? "historyBrowser.hideChanges" : "historyBrowser.showChanges")}</button>{expanded && <TurnFiles client={client} taskId={taskId} metadata={metadata} operation={item.kind === "activity"} />}</div> : data && <details><summary>{t("historyBrowser.details")}</summary><pre>{JSON.stringify(data, null, 2)}</pre></details>}
  </article>;
}
