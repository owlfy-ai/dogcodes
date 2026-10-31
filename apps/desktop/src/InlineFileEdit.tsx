import { useMemo, type ReactNode } from "react";
import { DeferredDetails } from "./DeferredDetails";
import { ActivityIcon } from "./ActivityIcon";
import { highlightedLines, languageForPath } from "./syntaxHighlight";
import { artifactVisibleText } from "./artifactView";
import type { DiffFile, DiffHunk } from "./diffView";
import "./inlineFileEdit.css";
import { translate, useI18n } from "./i18n";

function Hunk({ hunk, language }: { hunk: DiffHunk; language?: string }) {
  const rows = useMemo(() => {
    const before = highlightedLines(hunk.lines.filter(line => line.kind === "context" || line.kind === "deletion").map(line => artifactVisibleText(line.text)).join("\n"), language);
    const after = highlightedLines(hunk.lines.filter(line => line.kind === "context" || line.kind === "addition").map(line => artifactVisibleText(line.text)).join("\n"), language);
    let oldIndex = 0, newIndex = 0;
    return hunk.lines.map(line => {
      const old = line.kind === "deletion" || line.kind === "context" ? before[oldIndex++] : undefined;
      const next = line.kind === "addition" || line.kind === "context" ? after[newIndex++] : undefined;
      return { line, content: next ?? old ?? artifactVisibleText(line.text) };
    });
  }, [hunk, language]);
  return <>{rows.map(({ line, content }, index) => <div key={index} className={`inline-edit-line ${line.kind}`}>
    <span className="inline-edit-number" aria-label={line.kind === "deletion" ? translate("inlineEdit.oldLine", { line: line.oldLine ?? "" }) : translate("inlineEdit.newLine", { line: line.newLine ?? "" })}>{line.newLine ?? line.oldLine ?? ""}</span>
    <code>{content || " "}</code>
  </div>)}</>;
}

export function InlineFileEdit({ path, file, status, rawPatch, truncated, onOpen, openControl, running = false }: {
  path: string; file?: DiffFile; status?: string; rawPatch?: string; truncated?: boolean; onOpen?: () => void; openControl?: ReactNode; running?: boolean;
}) {
  const { t } = useI18n();
  const display = artifactVisibleText(path).replaceAll("\n", "\\n").replaceAll("\t", "\\t");
  const separator = Math.max(display.lastIndexOf("/"), display.lastIndexOf("\\"));
  const name = display.slice(separator + 1), directory = display.slice(0, separator + 1);
  const extension = name.split(".").pop()?.toLowerCase() ?? "";
  const label = t(file?.change === "added" ? "inlineEdit.added" : file?.change === "deleted" ? "inlineEdit.deleted" : file?.change === "renamed" ? "inlineEdit.renamed" : "inlineEdit.edited");
  return <DeferredDetails className="inline-file-edit" summary={
    <summary className={`inline-edit-heading${running ? " activity-running" : ""}`}>
      <ActivityIcon kind="edit" />
      <span className="inline-edit-label activity-text">{label}</span>
      <span className={`inline-edit-language lang-${extension}`} aria-hidden="true">{extension === "rs" ? "R" : extension === "java" ? "J" : extension.slice(0, 3).toUpperCase() || "◇"}</span>
      <span className="inline-edit-filename activity-text" data-tooltip={display}>{name}</span>
      <span className="inline-edit-directory activity-text" data-tooltip={directory}>{directory}</span>
      {file && !file.binary && <span className="inline-edit-counts">{file.additions > 0 && <span className="additions">+{file.additions}</span>}{file.deletions > 0 && <span className="deletions">−{file.deletions}</span>}</span>}
      {status && <span className="inline-edit-status activity-text">{status}</span>}
      <svg className="inline-edit-chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg>
    </summary>}>
    {() => <>
    {file?.oldPath && file.oldPath !== file.newPath && <p className="inline-edit-notice">{artifactVisibleText(file.oldPath)} → {artifactVisibleText(file.newPath)}</p>}
    {truncated && <p className="inline-edit-notice">{t("inlineEdit.truncated")}</p>}
    <div className="inline-edit-code" role="region" aria-label={t("inlineEdit.changesOf", { name })} tabIndex={0}>
      {file?.binary ? <p className="inline-edit-notice">{t("inlineEdit.binary")}</p>
        : file?.hunks.length ? <div className="inline-edit-lines">{file.hunks.map((hunk, index) => <div key={index}>
          {index > 0 && <div className="inline-edit-gap" data-tooltip={hunk.header}>···</div>}
          <Hunk hunk={hunk} language={languageForPath(path)} />
        </div>)}</div>
        : rawPatch ? <pre className="inline-edit-raw">{artifactVisibleText(rawPatch)}</pre>
        : <p className="inline-edit-notice">{t(file ? "inlineEdit.metadataOnly" : "inlineEdit.noDiff")}</p>}
    </div>
    {openControl ?? (onOpen && <button className="inline-edit-open" type="button" onClick={onOpen} aria-label={t("inlineEdit.openName", { name })}>{t("inlineEdit.openFile")} ↗</button>)}
    </>}
  </DeferredDetails>;
}
