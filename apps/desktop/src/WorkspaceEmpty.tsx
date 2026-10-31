import { translate as t } from "./i18n";
export function WorkspaceEmpty({ view }: { view: "files" | "changes" }) {
  return <div className="review-empty">
    <svg className="review-empty-mark" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {view === "changes" ? <><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Z"/><path d="M14 2v6h6M8 12h6m-3-3v6m-3 3h6"/></> : <><path d="M9 3h7l5 5v10H9V3Zm7 0v5h5M6 7H3v14h12v-3"/></>}
    </svg>
    <h3>{t(view === "changes" ? "workspaceEmpty.changes" : "workspaceEmpty.preview")}</h3>
    <p>{t("workspaceEmpty.hint")}</p>
  </div>;
}
