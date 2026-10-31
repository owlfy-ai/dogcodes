import type { FileVersion, ReviewChange } from "./reviewClient";
import { translate as t } from "./i18n";

function describe(version: FileVersion | null): string {
  if (!version) return t("fileMetadata.missing");
  if (version.kind === "gitlink") return t("fileMetadata.gitlink");
  const kind = t(version.kind === "symlink" ? "structuredDiff.mode.symlink" : version.binary ? "fileMetadata.binary" : "fileMetadata.text");
  return `${kind} · ${t("fileMetadata.bytes", { count: version.bytes.toLocaleString() })}${version.kind === "file" && version.executable ? ` · ${t("fileMetadata.executable")}` : ""}`;
}

/** These are Git-style type/executable modes, not a claim about full OS ACLs. */
export function ReviewFileMetadata({ change }: { change: ReviewChange }) {
  const sameContent = change.before && change.after && change.before.hash === change.after.hash;
  const symlink = change.before?.kind === "symlink" || change.after?.kind === "symlink";
  const typeChanged = change.before && change.after && change.before.kind !== change.after.kind;
  return <section className="review-file-metadata" aria-label={t("fileMetadata.label")}>
    <strong>{t(change.kind === "renamed" ? "fileMetadata.renamed" : change.kind === "added" ? "fileMetadata.added" : change.kind === "deleted" ? "approval.section.deleteFile" : "approval.category.editFile")}</strong>
    {change.previousPath && <p>{change.previousPath} → {change.path}</p>}
    <dl><dt>{t("fileMetadata.before")}</dt><dd>{describe(change.before)}</dd><dt>{t("fileMetadata.after")}</dt><dd>{describe(change.after)}</dd></dl>
    {sameContent && <p>{t(change.previousPath ? "fileMetadata.pathOnly" : "fileMetadata.typeOnly")}</p>}
    {(change.kind === "added" && change.after?.bytes === 0 || change.kind === "deleted" && change.before?.bytes === 0) && <p>{t("fileMetadata.emptyFile")}</p>}
    {symlink && <p>{t("fileMetadata.symlink")}</p>}
    {(change.before?.kind === "gitlink" || change.after?.kind === "gitlink") && <>
      <p>{t(typeChanged ? "fileMetadata.submoduleTypeChanged" : "fileMetadata.submodulePointer")}{t("fileMetadata.submoduleIndex")}</p>
      <p>{t("fileMetadata.submoduleOnly")}</p>
    </>}
    <small>{t("fileMetadata.modeNote")}</small>
  </section>;
}
