import { useEffect, useMemo, useRef, useState } from "react";
import type { DiffFile, StructuredDiff } from "./diffView";
import { artifactVisibleText } from "./artifactView";
import { FileIcon } from "./FileIcon";
import { ReviewBinaryPlaceholder } from "./ReviewBinaryPlaceholder";
import { ReviewFilesSplit } from "./ReviewFilesSplit";
import { ReviewSplitToggle } from "./ReviewSplitToggle";
import { ReviewText } from "./ReviewText";
import { reviewPatchHunk } from "./reviewPatch";
import { translate as t } from "./i18n";
import "./historicalDiff.css";

const visiblePath = (path: string) => artifactVisibleText(path).replace(/\n/g, "\\n").replace(/\t/g, "\\t");
const filePath = (file: DiffFile) => file.newPath || file.oldPath;
const statusFor = (file: DiffFile) => ({ added: "A", deleted: "D", renamed: "R", copied: "R", modified: "M", binary: "M" } as const)[file.change];

function HistoricalFileText({ file, split }: { file: DiffFile; split: boolean }) {
  const hunks = useMemo(() => file.hunks.map(hunk => ({ hunk, diff: reviewPatchHunk(hunk) })), [file]);
  return <>{hunks.map(({ hunk, diff }, index) => <section key={index}>
    <div className="review-context historical-diff-hunk">{artifactVisibleText(hunk.header)}</div>
    <ReviewText diff={diff} split={split} showStatistics={false} path={filePath(file)}
      lineOffsets={{ before: Math.max(0, hunk.oldStart - 1), after: Math.max(0, hunk.newStart - 1) }} />
  </section>)}</>;
}

export function HistoricalDiffView({ diff, initialPath }: { diff: StructuredDiff; initialPath?: string }) {
  const [selectedIndex, setSelectedIndex] = useState(() => Math.max(0, diff.files.findIndex(file => file.newPath === initialPath || file.oldPath === initialPath)));
  const [split, setSplit] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  const selected = diff.files[selectedIndex];
  useEffect(() => { if (body.current) body.current.scrollTop = 0; }, [selected]);
  return <section className="review-panel historical-diff-view" aria-label={t("structuredDiff.label")}>
    <div className="review-sidebar-header">
      <span className="review-sidebar-title">{t(diff.truncated ? "structuredDiff.filesPartial" : "structuredDiff.files", { count: diff.files.length })}
        <span className="review-total"><span className="review-add-count">+{diff.additions}</span><span className="review-delete-count">−{diff.deletions}</span></span>
      </span>
      <span className="review-sidebar-actions"><ReviewSplitToggle split={split} onToggle={() => setSplit(value => !value)} /></span>
    </div>
    {diff.truncated && <p className="review-tree-message" role="status">{t("structuredDiff.truncated", { reason: diff.truncationReason ?? "" })}</p>}
    {!diff.files.length ? <p className="review-tree-message">{t("structuredDiff.empty")}</p> : <ReviewFilesSplit active={Boolean(selected)} busy={false} files={
      <nav aria-label={t("structuredDiff.fileList")}><div className="review-root" data-view="list"><div className="review-root-content">
        {diff.files.map((file, index) => {
          const path = visiblePath(filePath(file)), status = statusFor(file);
          return <button type="button" className="review-file-entry" key={index} aria-label={path}
            aria-current={index === selectedIndex ? "true" : undefined} data-tooltip={path} onClick={() => setSelectedIndex(index)}>
            <FileIcon path={path} /><span className="review-entry-name">{path.split("/").at(-1)}</span>
            {!file.binary && <span className="review-entry-counts"><span className="review-add-count">+{file.additions}</span><span className="review-delete-count">−{file.deletions}</span></span>}
            <span className={`review-status status-${status}`} aria-label={t(`structuredDiff.change.${file.change}`)}>{status}</span>
          </button>;
        })}
      </div></div></nav>
    }>
      {selected && <div className="review-file-body" ref={body}>
        <div className="review-file-heading"><strong data-tooltip={visiblePath(filePath(selected))}>{visiblePath(filePath(selected))}</strong>
          {!selected.binary && <span className="review-entry-counts"><span className="review-add-count">+{selected.additions}</span><span className="review-delete-count">−{selected.deletions}</span></span>}
        </div>
        <details className="historical-diff-metadata" key={`${selectedIndex}:metadata`}>
          <summary>{t(`structuredDiff.change.${selected.change}`)}</summary>
          <dl>
            {selected.oldPath !== selected.newPath && <><dt>{t("structuredDiff.oldPath")}</dt><dd>{visiblePath(selected.oldPath)}</dd><dt>{t("structuredDiff.newPath")}</dt><dd>{visiblePath(selected.newPath)}</dd></>}
            {(selected.oldMode || selected.newMode) && <><dt>{t("structuredDiff.modeLabel")}</dt><dd>{selected.oldMode ?? "—"} → {selected.newMode ?? "—"}</dd></>}
            {selected.similarity && <><dt>{t("structuredDiff.similarityCheck")}</dt><dd>{t(selected.dissimilar ? "diff.dissimilarity" : "structuredDiff.similarity", { value: selected.similarity })}</dd></>}
          </dl>
        </details>
        {selected.binary ? <ReviewBinaryPlaceholder /> : !selected.hunks.length ? <p className="review-tree-message">{t("structuredDiff.metadataOnly")}</p>
          : <HistoricalFileText key={selectedIndex} file={selected} split={split} />}
      </div>}
    </ReviewFilesSplit>}
  </section>;
}
