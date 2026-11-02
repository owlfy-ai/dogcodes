import { permissionKind } from "./permissionPresentation";
import type { PermissionCatalog } from "@dogcodes/protocol";

type Mode = PermissionCatalog["modes"][number];

const shield = "m12 2 8 3.5V12c0 4.5-3.4 7.6-8 9.5C7.4 19.6 4 16.5 4 12V5.5Z";

export function PermissionIcon({ providerId, mode }: { providerId: string; mode?: Mode }) {
  const icon = permissionKind(providerId, mode);
  return <svg className="permission-icon" data-icon={icon} aria-hidden="true" width="18" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    {icon === "ask" ? <>
      <path d="M18 11V7a2 2 0 0 0-4 0M14 10V5a2 2 0 0 0-4 0v5M10 7a2 2 0 0 0-4 0v9"/>
      <path d="M18 8a2 2 0 0 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-1.1-6-3l-3.6-4.5a2 2 0 0 1 3.1-2.5L8 15"/>
    </> : icon === "onRequest" ? <>
      <path d="M21 12a8.5 8.5 0 0 1-12.4 7.5L3 21l1.5-5.6A8.5 8.5 0 1 1 21 12Z"/><path d="M10 9.5a2.2 2.2 0 1 1 3.2 2c-.7.4-1.2.9-1.2 1.8M12 16.5h.01"/>
    </> : icon === "edit" ? <>
      <path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16Z"/><path d="m13.5 6.5 4 4M13 20h7"/>
    </> : icon === "plan" ? <>
      <rect x="4" y="4" width="16" height="18" rx="2"/><path d="M8 2v5m4-5v5m4-5v5M8 11h8M8 15h8M8 19h5"/>
    </> : icon === "read" ? <>
      <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/>
    </> : icon === "auto" ? <>
      <path d="m11 3 1.9 5.1L18 10l-5.1 1.9L11 17l-1.9-5.1L4 10l5.1-1.9Z"/><path d="M18 15v6m-3-3h6"/>
    </> : icon === "deny" ? <>
      <circle cx="12" cy="12" r="9"/><path d="m5.6 5.6 12.8 12.8"/>
    </> : icon === "bypass" ? <>
      <path d={shield}/><path d="m3 3 18 18"/>
    </> : icon === "full" ? <>
      <rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 7.7-1.5M12 15v2"/>
    </> : <>
      <path d="M4 6h9m4 0h3M4 12h3m4 0h9M4 18h11m4 0h1"/><circle cx="15" cy="6" r="2"/><circle cx="9" cy="12" r="2"/><circle cx="17" cy="18" r="2"/>
    </>}
  </svg>;
}
