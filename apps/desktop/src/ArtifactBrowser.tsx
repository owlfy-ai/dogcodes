import { LoadingStatus } from "./LoadingStatus";
import { userFacingError } from "./userFacingError";
import { focusDialogTitle } from "./dialogFocus";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ArtifactCursor, CoreClient, TaskArtifactSummary, TaskId } from "@dogcodes/protocol";
import { acceptArtifactChunk, artifactPreview, verifyArtifactBytes } from "./artifactView";
import { parseGitDiff } from "./diffView";
import { StructuredDiffView } from "./StructuredDiffView";
import { translate as t } from "./i18n";

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
    }).catch(() => { if (active) setMessage(t("artifacts.listFailedClose")); })
      .finally(() => { if (active) setListBusy(false); });
    return () => { active = false; alive.current = false; generation.current += 1; dialog.current?.close(); };
  }, [client, taskId]);

  const loadMore = async () => {
    if (!cursor || listBusy) return;
    setListBusy(true);
    try {
      const page = await client.taskArtifacts(taskId, cursor);
      if (alive.current) { setItems((old) => [...old, ...page.items]); setCursor(page.nextCursor); }
    } catch { if (alive.current) setMessage(t("artifacts.listFailed")); }
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
        setVerified(false); setMessage(userFacingError(cause, t("artifacts.readFailed")));
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
      if (alive.current) setMessage(t(outcome === "saved" ? "artifacts.saved" : "artifacts.exportCancelled"));
    } catch (cause) { if (alive.current) setMessage(userFacingError(cause, t("artifacts.exportFailed"))); }
    finally { if (alive.current) setExportBusy(false); }
  };

  const preview = artifactPreview(bytes, verified, selected?.metadata.mediaType.endsWith("+json"));
  const isGitDiff = selected?.metadata.mediaType === "application/vnd.dogcodes.git-diff.v1.patch";
  const parsedDiff = useMemo(() => verified && isGitDiff ? parseGitDiff(bytes) : null, [bytes, isGitDiff, verified]);
  const showStructuredDiff = parsedDiff?.kind === "structured" && !rawDiffView;
  return <dialog className="artifact-dialog" ref={dialog} aria-labelledby="artifact-title"
    onCancel={(event) => { if (exportBusy) event.preventDefault(); else onClose(); }}>
    <header><div><p className="eyebrow">{t("artifacts.eyebrow")}</p><h2 id="artifact-title" tabIndex={-1} data-dialog-title>Task Artifacts</h2></div>
      <button type="button" aria-label={t("artifacts.close")} disabled={exportBusy} onClick={onClose}>×</button></header>
    <p className="artifact-warning">{t("artifacts.warning")}</p>
    <div className="artifact-layout">
      <nav aria-label={t("artifacts.listLabel")}>
        {!listBusy && items.length === 0 && <p>{t("artifacts.empty")}</p>}
        {items.map((item) => <button type="button" key={item.metadata.artifactId} disabled={exportBusy}
          className={selected?.metadata.artifactId === item.metadata.artifactId ? "active" : ""} onClick={() => choose(item)}>
          <b>{t(item.metadata.mediaType.endsWith(".patch") ? "artifacts.kind.diff" : item.metadata.mediaType.includes("test-report") ? "artifacts.kind.testReport" : item.sourceKind.endsWith(".original") ? "artifacts.kind.contextOriginal" : "artifacts.kind.contextConfirmed")}</b>
          <code>{item.metadata.artifactId}</code><small>{t("artifacts.event", { sequence: item.sourceSequence, bytes: item.metadata.byteLength })}</small>
        </button>)}
        {listBusy && <LoadingStatus>{t("artifacts.loadingMetadata")}</LoadingStatus>}
        {cursor && <button type="button" disabled={listBusy} onClick={() => void loadMore()}>{t("artifacts.older")}</button>}
      </nav>
      <section className="artifact-content" aria-label={t("artifacts.preview")}>
        {!selected ? <p>{t("artifacts.select")}</p> : <>
          <dl><dt>SHA-256</dt><dd><code>{selected.metadata.payloadSha256}</code></dd>
            <dt>{t("artifacts.mediaType")}</dt><dd>{selected.metadata.mediaType}</dd><dt>{t("artifacts.redaction")}</dt>
            <dd>{selected.metadata.redactionRulesetVersion === 0 ? t("artifacts.unredacted") : t("artifacts.redacted", { version: selected.metadata.redactionRulesetVersion, count: selected.metadata.redactionCount })}</dd></dl>
          <p role="status">{t("artifacts.readProgress", { read: bytes.length, total: selected.metadata.byteLength })}{` · ${t(verified ? "artifacts.hashVerified" : "artifacts.notFullyRead")}`}</p>
          {isGitDiff && verified && <div className="artifact-view-toggle" role="group" aria-label={t("artifacts.diffView")}>
            <button type="button" disabled={parsedDiff?.kind !== "structured"} className={showStructuredDiff ? "active" : ""} onClick={() => setRawDiffView(false)}>{t("artifacts.structured")}</button>
            <button type="button" className={!showStructuredDiff ? "active" : ""} onClick={() => setRawDiffView(true)}>{t("artifacts.raw")}</button>
          </div>}
          {parsedDiff?.kind === "fallback" && <p className="artifact-warning" role="status">{parsedDiff.reason}</p>}
          {showStructuredDiff && parsedDiff?.kind === "structured"
            ? <StructuredDiffView diff={parsedDiff.diff} artifactKey={selected.metadata.artifactId} />
            : <>{preview.hexadecimal && <p className="artifact-warning">{t("artifacts.notUtf8")}</p>}
              {readBusy && <LoadingStatus>{t("reviewPanel.reading")}</LoadingStatus>}{(preview.text || !readBusy) && <pre className="artifact-code" tabIndex={0}>{preview.text || t("artifacts.emptyContent")}</pre>}</>}
          {nextOffset !== null && <button type="button" disabled={readBusy} onClick={() => void read(selected, bytes, generation.current)}>{t("artifacts.readNext")}</button>}
          <label className="artifact-confirm"><input type="checkbox" checked={confirmed} disabled={!verified || exportBusy}
            onChange={(event) => setConfirmed(event.target.checked)} />{t("artifacts.confirmExport")}</label>
          <button type="button" disabled={!verified || !confirmed || exportBusy} onClick={() => void exportSelected()}>
            {t(exportBusy ? "artifacts.waitingSave" : "artifacts.export")}</button>
        </>}
      </section>
    </div>
    {message && <p role="status" className="artifact-warning">{message}</p>}
  </dialog>;
}
