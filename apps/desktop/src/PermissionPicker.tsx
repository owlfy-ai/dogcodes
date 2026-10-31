import { LoadingStatus } from "./LoadingStatus";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { FloatingPopover } from "./FloatingPopover";
import { DropdownChevron } from "./DropdownChevron";
import { focusComposerInput } from "./composerFocus";
import { useEffect, useRef, useState } from "react";
import type { ActiveRunSummary, CoreClient, ExecutorRef, PermissionCatalog, TaskId } from "@dogcodes/protocol";
import "./permissions.css";
import { permissionPresentation } from "./permissionPresentation";
import { permissionCatalogEntry } from "./permissionCatalogCache";
import { PermissionIcon } from "./PermissionIcon";
import { useI18n } from "./i18n";

export function PermissionPicker({client,executor,taskId,version,run,disabled,onChange}:{client:CoreClient;executor:ExecutorRef;taskId?:TaskId;version?:string|null;run?:ActiveRunSummary|null;disabled:boolean;onChange:(mode:string|null)=>void}) {
  const { t } = useI18n();
  const [catalog,setCatalog]=useState<PermissionCatalog|null>(null);
  const [selected,setSelected]=useState<string|null>(null);
  const [error,setError]=useState<"busy"|"unavailable"|null>(null);
  const [retry,setRetry]=useState(0);
  const [open,setOpen]=useState(false);
  const [updating,setUpdating]=useState(false);
  const [selectionError,setSelectionError]=useState<string|null>(null);
  const generation=useRef(0);
  const host=useRef<HTMLDivElement>(null);
  const change=useRef(onChange); change.current=onChange;
  const key=`dogcodes.permissions:${taskId}:${executor.providerId}:${executor.installationId}`;
  const liveSupported=Boolean(taskId && client.setRunningPermissionMode && executor.providerId==="cursor"
    && run?.executor.providerId===executor.providerId && run.executor.installationId===executor.installationId);
  const runReady=Boolean(run && ["running","waiting_for_approval","waiting_for_input"].includes(run.state));
  const blocked=disabled || updating || Boolean(run && (!liveSupported || !runReady));
  useEffect(()=>{
    generation.current++;
    setUpdating(false);setSelectionError(null);
    if(disabled) return;
    let live=true;
    setCatalog(null);setSelected(null);setError(null);setOpen(false);change.current(null);
    if (!client.permissionCatalog) { setCatalog({ modes: [], defaultModeId: null }); return; }
    const entry=permissionCatalogEntry(client,executor,taskId,version);
    const apply=(value:PermissionCatalog)=>{
      if (!live) return;
      let stored:string|null=null;try{stored=localStorage.getItem(key);}catch{/* storage may be unavailable */}
      // During a run the backend's actual policy wins over a saved next-turn choice.
      const mode=run&&liveSupported ? value.defaultModeId : value.modes.find(m=>m.id===stored)?.id??value.defaultModeId;
      setCatalog(value);setSelected(mode);change.current(mode);
    };
    const cached=run&&liveSupported ? undefined : entry.peek();
    if(cached) apply(cached);
    else (run&&liveSupported ? client.permissionCatalog(executor,taskId) : entry.get()).then(apply).catch(cause=>{if(live)setError(String(cause).includes("空闲") ? "busy" : "unavailable");});
    return ()=>{live=false;generation.current++;};
  },[client,key,version,retry,disabled,run?.runId,liveSupported,runReady]);
  useEffect(()=>{
    if(!open)return;
    host.current?.querySelector<HTMLButtonElement>('[aria-checked="true"]')?.focus();
    const click=(event:PointerEvent)=>{if(!host.current?.contains(event.target as Node))setOpen(false);};
    const escape=(event:KeyboardEvent)=>{if(event.key==="Escape"){setOpen(false);host.current?.querySelector<HTMLButtonElement>("button")?.focus();}};
    document.addEventListener("pointerdown",click);document.addEventListener("keydown",escape);
    return()=>{document.removeEventListener("pointerdown",click);document.removeEventListener("keydown",escape);};
  },[open]);
  useEffect(()=>{if(disabled||catalog&&!catalog.modes.length&&!error)setOpen(false);},[disabled,catalog,error]);
  const select=async(mode:string)=>{
    if(blocked)return;
    const currentGeneration=generation.current;
    setSelectionError(null);setUpdating(true);
    try {
      if(run) await client.setRunningPermissionMode!(taskId!,run.runId,executor,mode);
      // Resolving the last approval can remount the composer before the RPC
      // acknowledgement. Keep the accepted preference scoped to its original key.
      try{localStorage.setItem(key,mode);}catch{/* Selection remains valid without storage. */}
      if(generation.current!==currentGeneration)return;
      setSelected(mode);change.current(mode);setOpen(false);
      if (!focusComposerInput(host.current)) host.current?.querySelector<HTMLButtonElement>("button")?.focus();
    } catch(cause) {
      if(generation.current===currentGeneration)setSelectionError(userFacingError(cause, t("permissionPicker.retryLater")));
    } finally {
      if(generation.current===currentGeneration)setUpdating(false);
    }
  };
  // Present at render time so labels follow the current UI language.
  const modes=catalog?catalog.modes.map(item => permissionPresentation(executor.providerId, item)):[];
  const current=modes.find(m=>m.id===selected);
  const currentLabel = current?.label??t(error?error==="busy"?"permissionPicker.agentBusy":"permissionPicker.unavailable":catalog?"permissionPicker.defaultPermission":"permissionPicker.settings");
  // With nothing to choose, the trigger only reports the default policy instead of opening an empty menu.
  const fixed=Boolean(catalog && !catalog.modes.length && !error);
  return <div className="permission-picker" data-provider={executor.providerId} ref={host}>
    <button type="button" className={`permission-trigger ${current?.dangerous && executor.providerId !== "zcode"?"dangerous":""}`} disabled={blocked} aria-disabled={fixed||undefined} data-fixed={fixed||undefined}
      data-tooltip={run&&!liveSupported?t("permissionPicker.runningUnsupported"):fixed?t("permissionPicker.fixedTooltip"):t("permissionPicker.currentTooltip", { label: currentLabel })}
      aria-label={t("permissionPicker.title")} aria-expanded={fixed?undefined:open} aria-haspopup={fixed?undefined:"menu"} onClick={()=>{if(!fixed)setOpen(!open);}}>
      <PermissionIcon providerId={executor.providerId} mode={current} /> <span className="composer-control-label">{currentLabel}</span> {!fixed&&<DropdownChevron />}
    </button>
    {open&&<FloatingPopover className="permission-menu" role="menu" aria-label={t("permissionPicker.title")} onKeyDown={event=>{
      if(!["ArrowDown","ArrowUp","Home","End"].includes(event.key))return;
      event.preventDefault();
      const items=Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]'));
      const index=items.indexOf(document.activeElement as HTMLButtonElement);
      const next=event.key==="Home"?0:event.key==="End"?items.length-1:(index+(event.key==="ArrowUp"?-1:1)+items.length)%items.length;
      items[next]?.focus();
    }}>
      {error?<InlineError className="permission-error" onRetry={()=>setRetry(n=>n+1)}>{t(error==="busy"?"permissionPicker.busyError":"permissionPicker.loadError")}</InlineError>:!catalog?<LoadingStatus className="permission-empty">{t("permissionPicker.loading")}</LoadingStatus>:!modes.length?<div className="permission-empty">{t("permissionPicker.noModes")}</div>:modes.map(mode=><button type="button" role="menuitemradio" disabled={blocked} aria-checked={selected===mode.id} key={mode.id} className={mode.dangerous && executor.providerId !== "zcode"?"dangerous":""} onClick={()=>void select(mode.id)}><PermissionIcon providerId={executor.providerId} mode={mode} /><span><strong>{mode.label}</strong><small>{mode.description}</small></span><span aria-hidden="true">{selected===mode.id?"✓":""}</span></button>)}
      {selectionError&&<InlineError className="permission-error">{t("permissionPicker.notChanged", { reason: selectionError })}</InlineError>}
      <div className="permission-hint">{updating?<LoadingStatus>{t("permissionPicker.applying")}</LoadingStatus>:t(run?"permissionPicker.appliesNow":"permissionPicker.appliesNextTurn")}</div>
    </FloatingPopover>}
  </div>;
}
