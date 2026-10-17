// @vitest-environment jsdom
import {act} from "react";
import {createRoot, type Root} from "react-dom/client";
import {afterEach,beforeEach,expect,it,vi} from "vitest";
import {PermissionPicker} from "./PermissionPicker";
import type {ActiveRunSummary,CoreClient,PermissionCatalog} from "@dogcodes/protocol";
let host:HTMLDivElement,root:Root;
beforeEach(()=>{vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true);localStorage.clear();host=document.createElement("div");document.body.append(host);root=createRoot(host);});
afterEach(async()=>{await act(async()=>root.unmount());host.remove();vi.unstubAllGlobals();});
const executor={providerId:"claude",installationId:"ins_a"} as const;
const catalog:PermissionCatalog={defaultModeId:"default",modes:[{id:"default",label:"默认",description:"询问",dangerous:false},{id:"bypass",label:"跳过权限检查",description:"允许所有工具",dangerous:true}]};
it("selects the actual provider policy and scopes persistence to this task",async()=>{
 const change=vi.fn();const client={permissionCatalog:vi.fn(async()=>catalog)} as unknown as CoreClient;
 await act(async()=>root.render(<PermissionPicker client={client} executor={executor} taskId="tsk_a" disabled={false} onChange={change}/>));
 expect(change).toHaveBeenLastCalledWith("default");
 await act(async()=>host.querySelector<HTMLButtonElement>("button")!.click());
 const options=host.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]');
 expect(options[0].getAttribute("aria-checked")).toBe("true");
 await act(async()=>options[1].click());
 expect(change).toHaveBeenLastCalledWith("bypass");expect(host.querySelector('[role="menu"]')).toBeNull();
 expect(localStorage.getItem("dogcodes.permissions:tsk_a:claude:ins_a")).toBe("bypass");
 await act(async()=>root.render(<PermissionPicker client={client} executor={executor} taskId="tsk_b" disabled={false} onChange={change}/>));
 expect(change).toHaveBeenLastCalledWith("default");
});
it("discards stale stored policies and never guesses a policy after a read error",async()=>{
 localStorage.setItem("dogcodes.permissions:tsk_a:claude:ins_a","removed-mode");
 const change=vi.fn();const client={permissionCatalog:vi.fn(async()=>catalog)} as unknown as CoreClient;
 await act(async()=>root.render(<PermissionPicker client={client} executor={executor} taskId="tsk_a" disabled={false} onChange={change}/>));
 expect(change).toHaveBeenLastCalledWith("default");
 const failed={permissionCatalog:vi.fn(async()=>{throw Error("offline");})} as unknown as CoreClient;
 await act(async()=>root.render(<PermissionPicker client={failed} executor={executor} taskId="tsk_a" disabled={false} onChange={change}/>));
 expect(change).toHaveBeenLastCalledWith(null);expect(host.textContent).toContain("权限策略不可用");
});
it("shows a fixed default policy without an empty menu when nothing can be chosen",async()=>{
 const client={permissionCatalog:vi.fn(async()=>({defaultModeId:null,modes:[]}))} as unknown as CoreClient;
 await act(async()=>root.render(<PermissionPicker client={client} executor={executor} taskId="tsk_a" disabled={false} onChange={()=>{}}/>));
 const trigger=host.querySelector<HTMLButtonElement>("button")!;
 expect(trigger.getAttribute("aria-disabled")).toBe("true");expect(trigger.hasAttribute("aria-haspopup")).toBe(false);
 expect(trigger.dataset.tooltip).toContain("未提供可切换的权限策略");
 await act(async()=>trigger.click());
 expect(host.querySelector('[role="menu"]')).toBeNull();
});
it("prevents changing policy while a turn is running",async()=>{
 const client={permissionCatalog:vi.fn(async()=>catalog)} as unknown as CoreClient;
 await act(async()=>root.render(<PermissionPicker client={client} executor={executor} taskId="tsk_a" disabled onChange={()=>{}}/>));
 expect(host.querySelector<HTMLButtonElement>("button")!.disabled).toBe(true);
});

const cursor={providerId:"cursor",installationId:"ins_cursor"} as const;
const run:ActiveRunSummary={runId:"run_current",turnId:"trn_current",executor:cursor,state:"waiting_for_approval",startedAt:""};
const cursorCatalog:PermissionCatalog={defaultModeId:"default",modes:[
 {id:"default",label:"操作前确认",description:"",dangerous:false},
 {id:"full-access",label:"完全访问",description:"",dangerous:true},
]};
it("applies a running policy before updating the label and saved choice",async()=>{
 let finish!:()=>void;
 const update=vi.fn(()=>new Promise<void>(resolve=>{finish=resolve;}));
 const client={permissionCatalog:vi.fn(async()=>cursorCatalog),setRunningPermissionMode:update} as unknown as CoreClient;
 const change=vi.fn();
 await act(async()=>root.render(<PermissionPicker client={client} executor={cursor} taskId="tsk_a" run={run} disabled={false} onChange={change}/>));
 await act(async()=>host.querySelector<HTMLButtonElement>("button")!.click());
 await act(async()=>host.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')[1].click());
 expect(update).toHaveBeenCalledExactlyOnceWith("tsk_a","run_current",cursor,"full-access");
 expect(host.querySelector('.permission-trigger')?.textContent).toContain("操作前确认");
 expect(host.textContent).toContain("正在应用权限策略");
 expect(localStorage.getItem("dogcodes.permissions:tsk_a:cursor:ins_cursor")).toBeNull();
 await act(async()=>finish());
 expect(host.querySelector('.permission-trigger')?.textContent).toContain("完全访问");
 expect(change).toHaveBeenLastCalledWith("full-access");
 expect(localStorage.getItem("dogcodes.permissions:tsk_a:cursor:ins_cursor")).toBe("full-access");
});
it("keeps the actual running policy when the backend rejects a change",async()=>{
 const client={permissionCatalog:vi.fn(async()=>cursorCatalog),setRunningPermissionMode:vi.fn().mockRejectedValue(Error("运行已结束"))} as unknown as CoreClient;
 const change=vi.fn();
 localStorage.setItem("dogcodes.permissions:tsk_a:cursor:ins_cursor","full-access");
 await act(async()=>root.render(<PermissionPicker client={client} executor={cursor} taskId="tsk_a" run={run} disabled={false} onChange={change}/>));
 // A saved next-turn policy is not evidence that this run has full access.
 expect(host.querySelector('.permission-trigger')?.textContent).toContain("操作前确认");
 await act(async()=>host.querySelector<HTMLButtonElement>("button")!.click());
 await act(async()=>host.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')[1].click());
 expect(host.querySelector('[role="alert"]')?.textContent).toContain("运行已结束");
 expect(change).toHaveBeenLastCalledWith("default");
 expect(host.querySelector('.permission-trigger')?.textContent).toContain("操作前确认");
});
it("reads the current run's policy rather than another task's cached catalog",async()=>{
 const permissionCatalog=vi.fn().mockResolvedValueOnce(cursorCatalog).mockResolvedValueOnce({...cursorCatalog,defaultModeId:"full-access"});
 const client={permissionCatalog,setRunningPermissionMode:vi.fn()} as unknown as CoreClient;
 await act(async()=>root.render(<PermissionPicker client={client} executor={cursor} taskId="tsk_a" disabled={false} onChange={()=>{}}/>));
 await act(async()=>root.render(<PermissionPicker client={client} executor={cursor} taskId="tsk_b" run={run} disabled={false} onChange={()=>{}}/>));
 expect(permissionCatalog).toHaveBeenCalledTimes(2);
 expect(host.querySelector('.permission-trigger')?.textContent).toContain("完全访问");
});
it("disables live changes for an unsupported Agent or a different running executor",async()=>{
 const client={permissionCatalog:vi.fn(async()=>catalog),setRunningPermissionMode:vi.fn()} as unknown as CoreClient;
 await act(async()=>root.render(<PermissionPicker client={client} executor={executor} taskId="tsk_a" run={run} disabled={false} onChange={()=>{}}/>));
 expect(host.querySelector<HTMLButtonElement>("button")!.disabled).toBe(true);
 expect(client.setRunningPermissionMode).not.toHaveBeenCalled();
});
it("keeps an accepted preference on its original task when the picker remounts before acknowledgement",async()=>{
 let finish!:()=>void;
 const client={permissionCatalog:vi.fn(async()=>cursorCatalog),setRunningPermissionMode:vi.fn(()=>new Promise<void>(resolve=>{finish=resolve;}))} as unknown as CoreClient;
 const change=vi.fn();
 await act(async()=>root.render(<PermissionPicker key="old" client={client} executor={cursor} taskId="tsk_a" run={run} disabled={false} onChange={change}/>));
 await act(async()=>host.querySelector<HTMLButtonElement>("button")!.click());
 await act(async()=>host.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')[1].click());
 await act(async()=>root.render(<PermissionPicker key="new" client={client} executor={cursor} taskId="tsk_b" disabled={false} onChange={change}/>));
 await act(async()=>finish());
 expect(change).toHaveBeenLastCalledWith("default");
 expect(host.querySelector('.permission-trigger')?.textContent).toContain("操作前确认");
 expect(localStorage.getItem("dogcodes.permissions:tsk_a:cursor:ins_cursor")).toBe("full-access");
 expect(localStorage.getItem("dogcodes.permissions:tsk_b:cursor:ins_cursor")).toBeNull();
});
it("refreshes the effective policy once a starting run becomes ready",async()=>{
 const permissionCatalog=vi.fn().mockResolvedValueOnce(cursorCatalog).mockResolvedValueOnce({...cursorCatalog,defaultModeId:"full-access"});
 const client={permissionCatalog,setRunningPermissionMode:vi.fn()} as unknown as CoreClient;
 await act(async()=>root.render(<PermissionPicker client={client} executor={cursor} taskId="tsk_a" run={{...run,state:"starting"}} disabled={false} onChange={()=>{}}/>));
 expect(host.querySelector<HTMLButtonElement>("button")!.disabled).toBe(true);
 await act(async()=>root.render(<PermissionPicker client={client} executor={cursor} taskId="tsk_a" run={run} disabled={false} onChange={()=>{}}/>));
 expect(permissionCatalog).toHaveBeenCalledTimes(2);
 expect(host.querySelector('.permission-trigger')?.textContent).toContain("完全访问");
});
