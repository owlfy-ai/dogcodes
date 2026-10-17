import { LoadingStatus } from "./LoadingStatus";
import { userFacingError } from "./userFacingError";
import { focusDialogTitle } from "./dialogFocus";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ArtifactCursor, CoreClient, TaskArtifactSummary, TaskId } from "@dogcodes/protocol";
import { acceptArtifactChunk, artifactPreview, verifyArtifactBytes } from "./artifactView";
import { parseGitDiff } from "./diffView";
import { StructuredDiffView } from "./StructuredDiffView";

export type ArtifactReadClient = Pick<CoreClient, "taskArtifacts" | "readArtifact" | "exportArtifact">;

export function ArtifactBrowser({ client, taskId, onClose }: { client: ArtifactReadClient; taskId: TaskId; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const generation = useRef(0);
  const alive = useRef(true);
  const [items, setItems] = useState<TaskArtifactSummary[]>([]);
  const [cursor, setCursor] = useState<ArtifactCursor | null>(null);
  const [listBusy, setListBusy] = useState(true);
  const [selected, setSelected] = useState<TaskArtifactSummary | null>(null);
  const [bytes, setBytes] = useState<Uint8Array>(new Uint8Array());
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [readBusy, setReadBusy] = useState(false);
  const [verified, setVerified] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [exportBusy, setExportBusy] = useState(false);
  const [rawDiffView, setRawDiffView] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    alive.current = true;
    dialog.current?.showModal();
    focusDialogTitle(dialog.current);
    void client.taskArtifacts(taskId).then((page) => {
      if (active) { setItems(page.items); setCursor(page.nextCursor); }
    }).catch(() => { if (active) setMessage("Artifact 列表读取失败，请关闭后重试"); })
      .finally(() => { if (active) setListBusy(false); });
    return () => { active = false; alive.current = false; generation.current += 1; dialog.current?.close(); };
  }, [client, taskId]);

  const loadMore = async () => {
    if (!cursor || listBusy) return;
    setListBusy(true);
    try {
      const page = await client.taskArtifacts(taskId, cursor);
      if (alive.current) { setItems((old) => [...old, ...page.items]); setCursor(page.nextCursor); }
    } catch { if (alive.current) setMessage("Artifact 列表读取失败，可重试"); }
    finally { if (alive.current) setListBusy(false); }
  };

  const read = async (item: TaskArtifactSummary, current: Uint8Array, token: number) => {
    setReadBusy(true);
    setMessage(null);
    try {
      const chunk = await client.readArtifact({ taskId, artifactId: item.metadata.artifactId,
        expectedPayloadSha256: item.metadata.payloadSha256, offset: current.length, limit: 32768 });
      const combined = acceptArtifactChunk(item.metadata, current, chunk);
      if (chunk.nextOffset === null) await verifyArtifactBytes(combined, item.metadata.payloadSha256);
      if (alive.current && generation.current === token) {
        setBytes(combined); setNextOffset(chunk.nextOffset); setVerified(chunk.nextOffset === null);
      }
    } catch (cause) {
      if (alive.current && generation.current === token) {
        setVerified(false); setMessage(userFacingError(cause, "Artifact 读取失败，请重新选择后重试"));
      }
    } finally { if (alive.current && generation.current === token) setReadBusy(false); }
  };

  const choose = (item: TaskArtifactSummary) => {
    if (exportBusy) return;
    const token = ++generation.current;
    setSelected(item); setBytes(new Uint8Array()); setNextOffset(null); setVerified(false); setConfirmed(false); setRawDiffView(false);
    void read(item, new Uint8Array(), token);
  };

  const exportSelected = async () => {
    if (!selected || !verified || !confirmed || exportBusy) return;
    setExportBusy(true); setMessage(null);
    try {
      const outcome = await client.exportArtifact({ taskId, artifactId: selected.metadata.artifactId,
        expectedPayloadSha256: selected.metadata.payloadSha256, confirmedSensitiveExport: confirmed });
      if (alive.current) setMessage(outcome === "saved" ? "原始 Artifact 已保存" : "已取消导出，未写入文件");
    } catch (cause) { if (alive.current) setMessage(userFacingError(cause, "导出失败，请稍后重试。")); }
    finally { if (alive.current) setExportBusy(false); }
  };

  const preview = artifactPreview(bytes, verified, selected?.metadata.mediaType.endsWith("+json"));
  const isGitDiff = selected?.metadata.mediaType === "application/vnd.dogcodes.git-diff.v1.patch";
  const parsedDiff = useMemo(() => verified && isGitDiff ? parseGitDiff(bytes) : null, [bytes, isGitDiff, verified]);
  const showStructuredDiff = parsedDiff?.kind === "structured" && !rawDiffView;
  return <dialog className="artifact-dialog" ref={dialog} aria-labelledby="artifact-title"
    onCancel={(event) => { if (exportBusy) event.preventDefault(); else onClose(); }}>
    <header><div><p className="eyebrow">本机证据 · 只读</p><h2 id="artifact-title" tabIndex={-1} data-dialog-title>Task Artifacts</h2></div>
      <button type="button" aria-label="关闭 Artifact 查看" disabled={exportBusy} onClick={onClose}>×</button></header>
    <p className="artifact-warning">正文只在主动选择后读取；可能包含源码、个人信息和未脱敏内容。不会执行 HTML、Markdown 链接或 ANSI。</p>
    <div className="artifact-layout">
      <nav aria-label="当前任务的 Artifact">
        {!listBusy && items.length === 0 && <p>此任务尚无已固化 Artifact。</p>}
        {items.map((item) => <button type="button" key={item.metadata.artifactId} disabled={exportBusy}
          className={selected?.metadata.artifactId === item.metadata.artifactId ? "active" : ""} onClick={() => choose(item)}>
          <b>{item.metadata.mediaType.endsWith(".patch") ? "原始 Diff" : item.metadata.mediaType.includes("test-report") ? "测试报告" : item.sourceKind.endsWith(".original") ? "Context · Hub 原稿" : "Context · 已确认"}</b>
          <code>{item.metadata.artifactId}</code><small>事件 #{item.sourceSequence} · {item.metadata.byteLength} B</small>
        </button>)}
        {listBusy && <LoadingStatus>加载元数据…</LoadingStatus>}
        {cursor && <button type="button" disabled={listBusy} onClick={() => void loadMore()}>更早的 Artifact</button>}
      </nav>
      <section className="artifact-content" aria-label="Artifact 内容预览">
        {!selected ? <p>选择一条记录以校验并查看内容。</p> : <>
          <dl><dt>SHA-256</dt><dd><code>{selected.metadata.payloadSha256}</code></dd>
            <dt>媒体类型</dt><dd>{selected.metadata.mediaType}</dd><dt>脱敏</dt>
            <dd>{selected.metadata.redactionRulesetVersion === 0 ? "未脱敏（原始内容）" : `规则 v${selected.metadata.redactionRulesetVersion} · ${selected.metadata.redactionCount} 处替换；不保证无敏感内容`}</dd></dl>
          <p role="status">已读取 {bytes.length} / {selected.metadata.byteLength} B{verified ? " · 完整 hash 已校验" : " · 尚未完整读取"}</p>
          {isGitDiff && verified && <div className="artifact-view-toggle" role="group" aria-label="Diff 显示方式">
            <button type="button" disabled={parsedDiff?.kind !== "structured"} className={showStructuredDiff ? "active" : ""} onClick={() => setRawDiffView(false)}>结构化 Diff</button>
            <button type="button" className={!showStructuredDiff ? "active" : ""} onClick={() => setRawDiffView(true)}>原始内容</button>
          </div>}
          {parsedDiff?.kind === "fallback" && <p className="artifact-warning" role="status">{parsedDiff.reason}</p>}
          {showStructuredDiff && parsedDiff?.kind === "structured"
            ? <StructuredDiffView diff={parsedDiff.diff} artifactKey={selected.metadata.artifactId} />
            : <>{preview.hexadecimal && <p className="artifact-warning">不是有效 UTF-8：仅展示前 256 字节的十六进制。导出仍保留原始字节。</p>}
              {readBusy && <LoadingStatus>读取中…</LoadingStatus>}{(preview.text || !readBusy) && <pre className="artifact-code" tabIndex={0}>{preview.text || "（空内容）"}</pre>}</>}
          {nextOffset !== null && <button type="button" disabled={readBusy} onClick={() => void read(selected, bytes, generation.current)}>读取下一段（最多 32 KiB）</button>}
          <label className="artifact-confirm"><input type="checkbox" checked={confirmed} disabled={!verified || exportBusy}
            onChange={(event) => setConfirmed(event.target.checked)} />我已检查预览，确认导出原始内容（可能含敏感信息）。</label>
          <button type="button" disabled={!verified || !confirmed || exportBusy} onClick={() => void exportSelected()}>
            {exportBusy ? "等待保存…" : "选择新文件并导出原始字节"}</button>
        </>}
      </section>
    </div>
    {message && <p role="status" className="artifact-warning">{message}</p>}
  </dialog>;
}
