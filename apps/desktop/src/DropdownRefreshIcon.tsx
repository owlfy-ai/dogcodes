import "./dropdownRefreshIcon.css";

export function DropdownRefreshIcon({ refreshing }: { refreshing: boolean }) {
  return <svg className={`dropdown-refresh-icon${refreshing ? " is-refreshing" : ""}`} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M13 6a5 5 0 0 0-8.5-2L2 6m0-4v4h4m-3 4a5 5 0 0 0 8.5 2l2.5-2m0 4v-4h-4" />
  </svg>;
}
