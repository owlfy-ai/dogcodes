// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import type { ComputerUseActivity } from "@dogcodes/protocol";
const mocks = vi.hoisted(() => ({ invoke:vi.fn(), listeners:new Map<string,(event:{payload:unknown})=>void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke:mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen:vi.fn(async (name:string, callback:(event:{payload:unknown})=>void) => { mocks.listeners.set(name,callback); return () => mocks.listeners.delete(name); }) }));
import { ComputerUseNotice, ComputerUseNoticeView } from "./ComputerUseNotice";
const idle:ComputerUseActivity={operationId:null,taskId:null,tool:null,mode:"idle",preparing:false,foregroundPossible:false};

it("acknowledges the painted foreground glow and shows an Escape hint without controls", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.setItem("dogcodes.ui.language","en");
  const frames=new Map<number,FrameRequestCallback>(); let id=0;
  vi.stubGlobal("requestAnimationFrame",(callback:FrameRequestCallback)=>{frames.set(++id,callback);return id;});
  vi.stubGlobal("cancelAnimationFrame",(key:number)=>frames.delete(key));
  mocks.invoke.mockReset().mockImplementation(async (_command:string, args:{call:{type:string}}) => args.call.type==="status" ? idle : {accepted:true});
  const container=document.createElement("div"); const root=createRoot(container);
  const frame=()=>{const current=[...frames.values()];frames.clear();current.forEach(callback=>callback(0));};
  try {
    await act(async()=>root.render(<ComputerUseNotice/>));
    const activity:ComputerUseActivity={...idle,operationId:"op-foreground",taskId:"tsk_fixture",tool:"click",mode:"foreground",preparing:true};
    await act(async()=>mocks.listeners.get("computer-use-activity")!({payload:activity}));
    expect(container.textContent).toContain("Please pause input");
    expect(container.querySelector(".computer-glow-escape")!.textContent).toBe("Esc to exit");
    expect(container.querySelector("kbd")).toBeNull();
    expect(container.querySelector("button")).toBeNull();
    expect(mocks.invoke.mock.calls.filter(call=>call[1]?.call.type==="presented")).toHaveLength(0);
    await act(async()=>frame());
    expect(mocks.invoke.mock.calls.filter(call=>call[1]?.call.type==="presented")).toHaveLength(0);
    await act(async()=>frame());
    expect(mocks.invoke).toHaveBeenCalledWith("core_computer_notice",{call:{type:"presented",operationId:"op-foreground"}});
    await act(async()=>mocks.listeners.get("computer-use-disconnected")!({payload:null}));
    expect(container.textContent).toContain("Input state is unknown");
    await act(async()=>mocks.listeners.get("computer-use-activity")!({payload:idle}));
    expect(container.querySelector("aside")!.getAttribute("aria-hidden")).toBe("true");
    expect(container.querySelector("button")).toBeNull();
  } finally { await act(async()=>root.unmount()); localStorage.clear();vi.unstubAllGlobals(); }
});

it("uses orange for possible foreground input and green without Escape for background work", async()=>{
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true);
  const container=document.createElement("div");const root=createRoot(container);
  try {
    await act(async()=>root.render(<ComputerUseNoticeView activity={{...idle,mode:"foreground",foregroundPossible:true}}/>));
    expect(container.textContent).toContain("may use your keyboard and mouse");
    expect(container.querySelector("aside")!.classList.contains("computer-glow-foreground")).toBe(true);
    await act(async()=>root.render(<ComputerUseNoticeView activity={{...idle,mode:"background"}}/>));
    expect(container.textContent).toContain("verifying your computer in the background");
    expect(container.querySelector("aside")!.classList.contains("computer-glow-foreground")).toBe(false);
    expect(container.querySelector("kbd")).toBeNull();
    expect(container.querySelector(".computer-glow-escape")).toBeNull();
    expect(container.querySelector("button")).toBeNull();
  } finally {await act(async()=>root.unmount());vi.unstubAllGlobals();}
});

it("moves the caption below a camera housing while keeping the light on the top edge", async()=>{
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true);
  const container=document.createElement("div");const root=createRoot(container);
  try {
    await act(async()=>root.render(<ComputerUseNoticeView activity={{...idle,mode:"foreground"}} captionTop={45}/>));
    const glow=container.querySelector("aside")!;
    expect(glow.style.getPropertyValue("--computer-caption-top")).toBe("45px");
    expect(glow.classList.contains("computer-glow-safe-area")).toBe(true);
    expect(container.querySelector(".computer-glow-edge")).not.toBeNull();
    expect(container.querySelector("button, kbd")).toBeNull();
  } finally {await act(async()=>root.unmount());vi.unstubAllGlobals();}
});
