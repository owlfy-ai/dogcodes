import { permissionKind } from "./permissionPresentation";
import type { PermissionCatalog } from "@dogcodes/protocol";

type Mode = PermissionCatalog["modes"][number];

export function PermissionIcon({ providerId, mode }: { providerId: string; mode?: Mode }) {
  const icon = permissionKind(providerId, mode) ?? (mode?.dangerous ? "full" : "configured");
  return <svg className="permission-icon" aria-hidden="true" width="18" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    {icon === "ask" ? <>
      <path d="M18 11V7a2 2 0 0 0-4 0M14 10V5a2 2 0 0 0-4 0v5M10 7a2 2 0 0 0-4 0v9"/>
      <path d="M18 8a2 2 0 0 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-1.1-6-3l-3.6-4.5a2 2 0 0 1 3.1-2.5L8 15"/>
    </> : icon === "plan" ? <>
      <rect x="4" y="4" width="16" height="18" rx="2"/><path d="M8 2v5m4-5v5m4-5v5M8 11h8M8 15h8M8 19h5"/>
    </> : icon === "read" ? <>
      <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/>
    </> : <>
      <path d="m12 2 8 3.5V12c0 4.5-3.4 7.6-8 9.5C7.4 19.6 4 16.5 4 12V5.5Z"/>
      {icon === "edit" && <path d="m8.5 11.5 2.5 2.5 4.5-5"/>}
      {icon === "full" && <path d="M12 7v6m0 4h.01"/>}
      {icon === "auto" && <path d="m13 6-5 7h4l-1 5 5-8h-4Z"/>}
      {icon === "configured" && <><path d="M8 9h8M8 14h8M10 7v4M14 12v4"/></>}
    </>}
  </svg>;
}
