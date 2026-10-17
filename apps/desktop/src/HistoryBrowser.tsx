import { LoadingStatus } from "./LoadingStatus";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { userFacingError } from "./userFacingError";
import { SentAttachments } from "./attachments";
import { TurnFiles } from "./TurnFiles";
import { ArtifactBrowser } from "./ArtifactBrowser";
import { useEffect, useRef, useState } from "react";
import type { ArtifactMetadata, CoreClient, HistoryPage, HistoryQuery, TaskId } from "@dogcodes/protocol";
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
      if (!client.queryHistory) throw new Error("历史检索需要桌面应用。");
      const result = await client.queryHistory({taskId, kind, text: query, afterSequence: more ? page?.nextCursor ?? 0 : 0, throughSequence: more ? page?.throughSequence : null, limit: 30});
      if (id !== generation.current) return;
      setPage(result); setItems(old => more ? [...old, ...result.items] : result.items);
    } catch (e) { if (id === generation.current) setError(userFacingError(e, "暂时无法读取对话历史，请重试。")); }
    finally { if (id === generation.current) setBusy(false); }
  }
  useEffect(() => { setItems([]); setPage(null); void load(); return () => { generation.current++; }; }, [client, taskId, kind, query]);
  return <section className="history-browser">
    <button type="button" onClick={onBack}>← 返回对话</button>
    <h2>对话历史</h2>
    <button type="button" onClick={() => setArtifactsOpen(true)}>查看附件与文件变更</button>
    {artifactsOpen && <ArtifactBrowser key={taskId} client={client} taskId={taskId} onClose={() => setArtifactsOpen(false)} />}
    <form onSubmit={e => { e.preventDefault(); setKind(text.trim() ? "search" : "turns"); setQuery(text.trim()); }}>
      <input ref={searchInput} aria-label="搜索当前对话历史" placeholder="搜索当前对话…" value={text} onChange={e => setText(e.target.value)} />
      <button type="submit">搜索</button>
    </form>
    <nav aria-label="历史类型">{([["turns","对话"],["decisions","决策"],["changes","文件变更"]] as const).map(([value,label]) => <button key={value} type="button" aria-pressed={kind === value} onClick={() => {setKind(value); setQuery(""); setText("");}}>{label}</button>)}</nav>
    <p>搜索此对话中已保存的消息和操作记录，不包含思考过程或附件内容。</p>
    {items.map(item => <HistoryRecord key={`${taskId}:${item.sequence}`} item={item} client={client} taskId={taskId} />)}
    {!busy && !items.length && !error && <ReviewEmptyState kind={query ? "search" : "history"} compact
      title={page?.nextCursor != null ? "本段没有匹配记录" : query ? "没有找到相关内容" : kind === "decisions" ? "暂无决策记录" : kind === "changes" ? "暂无文件变更记录" : "暂无对话记录"}
      description={page?.nextCursor != null ? "还有历史内容尚未检索，可继续查找。" : query ? "试试其他关键词，或清空搜索查看对话历史。" : "这里还没有记录，可以返回对话继续交流。"}
      action={<>{page?.nextCursor != null && <button type="button" className="review-empty-action" onClick={() => void load(true)}>继续查找</button>}{query ? <button type="button" className={`review-empty-action${page?.nextCursor != null ? " review-empty-action--secondary" : ""}`} onClick={() => { setText(""); setQuery(""); setKind("turns"); searchInput.current?.focus(); }}>清空搜索</button> : page?.nextCursor == null && <button type="button" className="review-empty-action" onClick={onBack}>返回对话</button>}</>} />}
    {error && <p role="alert">{error} <button type="button" onClick={() => void load(items.length > 0)}>重试</button></p>}
    {busy && <LoadingStatus layout={items.length ? "inline" : "panel"}>正在查询…</LoadingStatus>}
    {page?.nextCursor != null && items.length > 0 && <button disabled={busy} type="button" onClick={() => void load(true)}>加载下一页</button>}
  </section>;
}

function HistoryRecord({ item, client, taskId }: { item: HistoryPage["items"][number]; client: CoreClient; taskId: TaskId }) {
  const [expanded, setExpanded] = useState(false);
  const labels: Record<string, string> = {user_message:"你",assistant_message:"Agent",system:"系统",activity:"执行记录",turn_files:"文件变更","decision.recorded":"决策","workspace.turn_changes_captured":"文件变更","workspace.turn_changes_failed":"变更采集失败"};
  let data: Record<string, unknown> | null = null;
  if (!["user_message", "assistant_message", "system"].includes(item.kind)) {
    try { const value: unknown = JSON.parse(item.text); if (value && typeof value === "object" && !Array.isArray(value)) data = value as Record<string, unknown>; } catch { /* Plain text records remain literal. */ }
  }
  const candidate = data?.artifact ?? (item.kind === "turn_files" ? data : null);
  const metadata = candidate && typeof candidate === "object" && "artifactId" in candidate && "payloadSha256" in candidate && "byteLength" in candidate && "mediaType" in candidate && typeof candidate.mediaType === "string" && /\.(?:turn|operation)-changes\.v[12]\+json$/.test(candidate.mediaType) ? candidate as ArtifactMetadata : null;
  const summary = data ? [data.statement, data.message, data.summary, data.title, data.name].find(value => typeof value === "string") as string | undefined : item.text;
  return <article>
    <small>{labels[item.kind] ?? "历史记录"} · {new Date(item.occurredAt).toLocaleString()}</small>
    {summary && <pre>{summary}</pre>}
    {item.kind === "user_message" && <SentAttachments items={item.attachments ?? []} />}
    {metadata ? <div><button type="button" onClick={() => setExpanded(value => !value)}>{expanded ? "收起变更" : "查看文件变更"}</button>{expanded && <TurnFiles client={client} taskId={taskId} metadata={metadata} operation={item.kind === "activity"} />}</div> : data && <details><summary>查看记录详情</summary><pre>{JSON.stringify(data, null, 2)}</pre></details>}
  </article>;
}
