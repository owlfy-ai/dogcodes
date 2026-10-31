import { ReviewEmptyState } from "./ReviewEmptyState";
import { useMemo } from "react";
import { MarkdownMessage } from "./MarkdownMessage";
import { highlightedLines, languageForPath } from "./syntaxHighlight";
import { ReviewBinaryPlaceholder } from "./ReviewBinaryPlaceholder";
import type { FilePreview } from "./reviewClient";
import { translate as t } from "./i18n";
function Code({ text, path }: { text: string; path: string }) {
  const lines = useMemo(() => highlightedLines(text, languageForPath(path)), [text, path]);
  return <pre className="review-code workspace-code"><code>{lines.map((line, index) => <span className="workspace-code-line" key={index}><span className="workspace-line-number" aria-hidden="true">{index + 1}</span><span>{line}{"\n"}</span></span>)}</code></pre>;
}

export function WorkspaceFilePreview({ preview, path }: { preview: FilePreview; path: string }) {
  if (preview.text !== null && languageForPath(path) === "markdown" && !preview.text.trim()) {
    return <div className="workspace-markdown-empty"><ReviewEmptyState kind="file" title={t("filePreview.empty")} description={t("filePreview.emptyDescription")} /></div>;
  }
  return preview.text !== null ? languageForPath(path) === "markdown" ? <div className="workspace-markdown"><MarkdownMessage text={preview.text} /></div> : <Code text={preview.text} path={path} /> : <ReviewBinaryPlaceholder afterImage={preview.image} />;
}
