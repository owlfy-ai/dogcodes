import { translate as t } from "./i18n";
export function SidebarWindowControls({ collapsed, onToggle, canBack, canForward, onBack, onForward }: {
  collapsed: boolean; onToggle: () => void; canBack: boolean; canForward: boolean; onBack: () => void; onForward: () => void;
}) {
  return <div className="sidebar-window-controls">
    <button className="sidebar-toggle" type="button" aria-label={t(collapsed ? "sidebar.expand" : "sidebar.collapse")} data-tooltip={t(collapsed ? "sidebar.expand" : "sidebar.collapse")} aria-expanded={!collapsed} aria-controls="app-sidebar" onClick={onToggle}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="3" /><path d="M9 4v16" /></svg></button>
    <div className="history-controls"><button type="button" aria-label={t("sidebar.back")} data-tooltip={t("sidebar.back")} disabled={!canBack} onClick={onBack}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m10 6-6 6 6 6M4 12h16" /></svg></button><button type="button" aria-label={t("sidebar.forward")} data-tooltip={t("sidebar.forward")} disabled={!canForward} onClick={onForward}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m14 6 6 6-6 6M4 12h16" /></svg></button></div>
  </div>;
}
