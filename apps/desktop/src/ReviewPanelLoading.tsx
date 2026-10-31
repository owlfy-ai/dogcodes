import { translate as t } from "./i18n";
/**
 * Side panels show their layout to come, never the running dog: that is kept for the
 * main area, so two loading panels side by side don't compete. The cover hides stale
 * content at once; the skeleton itself appears only after the shared delay.
 */
export function ReviewPanelLoading({ showControls = true, visible = true }: { showControls?: boolean; visible?: boolean }) {
  return <div className="review-panel-loading" role="status" aria-label={t("reviewPanel.loadingDiff")}>
    {visible && <div className="review-panel-skeleton" aria-hidden="true">
      {showControls && <div className="review-loading-controls"><i /><i /></div>}
      <div className="review-loading-toolbar"><i /><span><b /><b /><b /></span></div>
      <div className="review-loading-list">
        <div className="review-loading-search" />
        {[68, 52, 76, 44, 61, 57].map((width, index) => <div className="review-loading-file" key={index}><b /><i style={{ width: `${width}%` }} /></div>)}
      </div>
    </div>}
  </div>;
}
