import { useEffect, useState } from "react";
import type { StructuredDiff } from "./diffView";
import { artifactVisibleText } from "./artifactView";
import { translate as t } from "./i18n";

const changeLabels: Record<StructuredDiff["files"][number]["change"], string> = {
  modified: "modified", added: "added", deleted: "deleted", renamed: "renamed", copied: "copied", binary: "binary",
};

const visiblePath = (path: string) => artifactVisibleText(path).replace(/\n/g, "\\n").replace(/\t/g, "\\t");
const modeLabels: Record<string, string> = { "100644": "regular", "100755": "executable", "120000": "symlink", "160000": "submodule" };
const modeLabel = (mode: string | null) => mode === null ? "—" : `${mode} ${modeLabels[mode] ? t(`structuredDiff.mode.${modeLabels[mode]}`) : ""}`;

export function StructuredDiffView({ diff, artifactKey, initialPath }: { diff: StructuredDiff; artifactKey: string; initialPath?: string }) {
  const [selectedIndex, setSelectedIndex] = useState(0);
  useEffect(() => setSelectedIndex(Math.max(0, diff.files.findIndex(file => file.newPath === initialPath || file.oldPath === initialPath))), [artifactKey, initialPath, diff.files]);
  const selected = diff.files[selectedIndex] ?? null;

  return <section className="diff-view" aria-label={t("structuredDiff.label")}>
    <header className="diff-summary">
      <b>{t(diff.truncated ? "structuredDiff.filesPartial" : "structuredDiff.files", { count: diff.files.length })}</b><span>{t("structuredDiff.text")}</span><span className="diff-add">+{diff.additions}</span><span className="diff-delete">−{diff.deletions}</span>
    </header>
    {diff.truncated && <p className="artifact-warning" role="status">{t("structuredDiff.truncated", { reason: diff.truncationReason ?? "" })}</p>}
    {diff.files.length === 0 ? <p>{t("structuredDiff.empty")}</p> : <div className="diff-layout">
      <nav aria-label={t("structuredDiff.fileList")}>
        {diff.files.map((file, index) => <button type="button" key={`${index}:${file.oldPath}:${file.newPath}`}
          className={index === selectedIndex ? "active" : ""} onClick={() => setSelectedIndex(index)}>
          <span>{visiblePath(file.newPath || file.oldPath)}</span>
          <small>{t(`structuredDiff.change.${changeLabels[file.change]}`)} · {file.binary ? t("structuredDiff.binaryContent") : `+${file.additions} −${file.deletions}`}</small>
        </button>)}
      </nav>
      {selected && <div className="diff-file">
        <header><b>{visiblePath(selected.newPath || selected.oldPath)}</b><span>{changeLabels[selected.change]}</span></header>
        {(selected.oldPath !== selected.newPath || selected.oldMode || selected.newMode || selected.similarity) && <dl>
          {selected.oldPath !== selected.newPath && <><dt>{t("structuredDiff.oldPath")}</dt><dd>{visiblePath(selected.oldPath)}</dd><dt>{t("structuredDiff.newPath")}</dt><dd>{visiblePath(selected.newPath)}</dd></>}
          {(selected.oldMode || selected.newMode) && <><dt>{t("structuredDiff.modeLabel")}</dt><dd>{selected.oldMode === selected.newMode ? modeLabel(selected.oldMode) : `${modeLabel(selected.oldMode)} → ${modeLabel(selected.newMode)}`}</dd></>}
          {selected.similarity && <><dt>{t("structuredDiff.similarityCheck")}</dt><dd>{artifactVisibleText(t(selected.dissimilar ? "diff.dissimilarity" : "structuredDiff.similarity", { value: selected.similarity }))}</dd></>}
        </dl>}
        {selected.binary ? <p className="artifact-warning">{t("structuredDiff.binary")}</p>
          : selected.hunks.length === 0 ? <p>{t("structuredDiff.metadataOnly")}</p>
          : <div className="diff-hunks"><p className="diff-column-labels">{t("structuredDiff.columns")}</p>{selected.hunks.map((hunk, hunkIndex) => <section key={`${hunk.header}:${hunkIndex}`}>
            <h3>{artifactVisibleText(hunk.header)}</h3>
            {hunk.lines.map((line, lineIndex) => <div className={`diff-line ${line.kind}`} key={lineIndex}>
              <span>{line.oldLine ?? ""}</span><span>{line.newLine ?? ""}</span>
              <code>{line.kind === "addition" ? "+" : line.kind === "deletion" ? "−" : line.kind === "context" ? " " : ""}{artifactVisibleText(line.text)}</code>
            </div>)}
          </section>)}</div>}
      </div>}
    </div>}
  </section>;
}
