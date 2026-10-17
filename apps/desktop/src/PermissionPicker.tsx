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

export function PermissionPicker({client,executor,taskId,version,run,disabled,onChange}:{client:CoreClient;executor:ExecutorRef;taskId?:TaskId;version?:string|null;run?:ActiveRunSummary|null;disabled:boolean;onChange:(mode:string|null)=>void}) {
  const [catalog,setCatalog]=useState<PermissionCatalog|null>(null);
  const [selected,setSelected]=useState<string|null>(null);
  const [error,setError]=useState<string|null>(null);
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
      setCatalog({...value, modes: value.modes.map(item => permissionPresentation(executor.providerId, item))});setSelected(mode);change.current(mode);
    };
    const cached=run&&liveSupported ? undefined : entry.peek();
    if(cached) apply(cached);
    else (run&&liveSupported ? client.permissionCatalog(executor,taskId) : entry.get()).then(apply).catch(cause=>{if(live)setError(String(cause).includes("空闲") ? "Agent 正忙，请在当前对话完成后重试。" : "无法读取当前 Agent 的权限策略。");});
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
      if(generation.current===currentGeneration)setSelectionError(userFacingError(cause, "请稍后重试。"));
    } finally {
      if(generation.current===currentGeneration)setUpdating(false);
    }
  };
  const current=catalog?.modes.find(m=>m.id===selected);
  const currentLabel = current?.label??(error?error.includes("正忙")?"Agent 忙碌中":"权限策略不可用":catalog?"默认权限":"权限设置");
  // With nothing to choose, the trigger only reports the default policy instead of opening an empty menu.
  const fixed=Boolean(catalog && !catalog.modes.length && !error);
  return <div className="permission-picker" data-provider={executor.providerId} ref={host}>
    <button type="button" className={`permission-trigger ${current?.dangerous && executor.providerId !== "zcode"?"dangerous":""}`} disabled={blocked} aria-disabled={fixed||undefined} data-fixed={fixed||undefined}
      data-tooltip={run&&!liveSupported?"当前 Agent 运行中暂不支持修改权限策略":fixed?"当前 Agent 未提供可切换的权限策略，将使用默认权限":`权限策略：${currentLabel}`}
      aria-label="权限策略" aria-expanded={fixed?undefined:open} aria-haspopup={fixed?undefined:"menu"} onClick={()=>{if(!fixed)setOpen(!open);}}>
      <PermissionIcon providerId={executor.providerId} mode={current} /> <span className="composer-control-label">{currentLabel}</span> {!fixed&&<DropdownChevron />}
    </button>
    {open&&<FloatingPopover className="permission-menu" role="menu" aria-label="权限策略" onKeyDown={event=>{
      if(!["ArrowDown","ArrowUp","Home","End"].includes(event.key))return;
      event.preventDefault();
      const items=Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]'));
      const index=items.indexOf(document.activeElement as HTMLButtonElement);
      const next=event.key==="Home"?0:event.key==="End"?items.length-1:(index+(event.key==="ArrowUp"?-1:1)+items.length)%items.length;
      items[next]?.focus();
    }}>
      {error?<InlineError className="permission-error" onRetry={()=>setRetry(n=>n+1)}>{error}</InlineError>:!catalog?<LoadingStatus className="permission-empty">正在获取权限设置…</LoadingStatus>:!catalog.modes.length?<div className="permission-empty">当前 Agent 未提供可切换的权限策略。</div>:catalog.modes.map(mode=><button type="button" role="menuitemradio" disabled={blocked} aria-checked={selected===mode.id} key={mode.id} className={mode.dangerous && executor.providerId !== "zcode"?"dangerous":""} onClick={()=>void select(mode.id)}><PermissionIcon providerId={executor.providerId} mode={mode} /><span><strong>{mode.label}</strong><small>{mode.description}</small></span><span aria-hidden="true">{selected===mode.id?"✓":""}</span></button>)}
      {selectionError&&<InlineError className="permission-error">权限策略未修改：{selectionError}</InlineError>}
      <div className="permission-hint">{updating?<LoadingStatus>正在应用权限策略…</LoadingStatus>:run?"应用成功后立即对当前运行生效":"所选策略在下一轮执行时生效"}</div>
    </FloatingPopover>}
  </div>;
}
