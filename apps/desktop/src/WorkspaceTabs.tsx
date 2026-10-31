import { translate as t } from "./i18n";
export type WorkspaceTab = "files" | "changes" | "run";

const tabs: { id: WorkspaceTab; label: string; path: string }[] = [
  { id: "changes", label: "reviewPanel.tab.changes", path: "M6 8v8m0-8a3 3 0 1 0 0-6 3 3 0 0 0 0 6Zm0 14a3 3 0 1 0 0-6 3 3 0 0 0 0 6Zm3-3c6 0 9-5 9-10m0 0a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z" },
  { id: "files", label: "reviewPanel.tab.files", path: "M9 3h7l5 5v10H9V3Zm7 0v5h5M6 7H3v14h12v-3" },
  { id: "run", label: "reviewPanel.tab.run", path: "M6 3l15 9-15 9Z" },
];

export function WorkspaceTabs({ value, onChange }: { value: WorkspaceTab; onChange: (tab: WorkspaceTab) => void }) {
  return <div className="workspace-tabs" role="tablist" aria-label={t("workspaceTabs.label")}>
    {tabs.map(({ id, label, path }, index) => <button key={id} type="button" role="tab" aria-label={t(label)} data-tooltip={t(label)} aria-selected={value === id} tabIndex={value === id ? 0 : -1} onClick={() => onChange(id)} onKeyDown={event => {
      const next = event.key === "ArrowRight" ? (index + 1) % tabs.length : event.key === "ArrowLeft" ? (index + tabs.length - 1) % tabs.length : event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : -1;
      if (next < 0) return;
      event.preventDefault();
      onChange(tabs[next].id);
      (event.currentTarget.parentElement?.children[next] as HTMLButtonElement)?.focus();
    }}><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={path} /></svg></button>)}
  </div>;
}
