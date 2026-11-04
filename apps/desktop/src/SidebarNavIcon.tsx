export function SidebarNavIcon({ kind }: { kind: "conversation" | "agent" | "quota" | "phone" | "server" }) {
  return <svg className="sidebar-nav-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    {kind === "conversation" ? <>
      <circle cx="12" cy="12" r="9" />
      <path d="M8 12h8m-4-4v8" />
    </> : kind === "phone" ? <>
      <rect x="6" y="2.5" width="12" height="19" rx="3" />
      <path d="M10 5.5h4M11 18.5h2" />
    </> : kind === "server" ? <>
      <rect x="3" y="4" width="18" height="7" rx="2" />
      <rect x="3" y="13" width="18" height="7" rx="2" />
      <path d="M7 7.5h.01M7 16.5h.01" />
    </> : kind === "agent" ? <>
      <rect x="3" y="5" width="18" height="16" rx="4" />
      <path d="M12 2v3M8 11v2m8-2v2m-7 4h6" />
    </> : <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 3v9h9" />
    </>}
  </svg>;
}
