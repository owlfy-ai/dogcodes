// @vitest-environment jsdom
import { act, createElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { describe, it, expect, vi } from "vitest";
const { invoke } = vi.hoisted(() => ({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke}));
vi.mock("qrcode",()=>({default:{toDataURL:vi.fn(async()=>"data:image/png;base64,fixture")}}));
import { RemoteDevices } from "./RemoteDevices";
describe("remote device pairing",()=>{
  it("closes the same dialog identity that created its invitation",async()=>{
    (globalThis as Record<string,unknown>).IS_REACT_ACT_ENVIRONMENT=true;
    invoke.mockResolvedValue({configured:false,connected:false,devices:[]});
    const node=document.createElement("div");document.body.append(node);const root=createRoot(node);
    await act(async()=>{root.render(createElement(StrictMode,null,createElement(RemoteDevices,{onClose:()=>{}})));});
    const button=[...node.querySelectorAll("button")].find(b=>b.textContent==="显示配对二维码")!;
    await act(async()=>{button.click();});
    const created=invoke.mock.calls.find(([,args])=>args?.call?.type==="invite")![1].call;
    expect(created.dialogId).toBeTruthy();
    const canceledBeforeSend=invoke.mock.calls.filter(([,args])=>args?.call?.type==="cancel_invitation").map(([,args])=>args.call.dialogId);
    expect(canceledBeforeSend).not.toContain(created.dialogId);
    await act(async()=>{root.unmount();});
    expect(invoke).toHaveBeenCalledWith("core_remote_control",{call:{type:"cancel_invitation",dialogId:created.dialogId}});
    node.remove();
  });
  it("shows a connection failure instead of an endless connecting status", async () => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    invoke.mockResolvedValue({ configured: true, connected: false, devices: [], error: "电脑登记凭证已失效" });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    await act(async () => { root.render(createElement(RemoteDevices, { onClose: () => {} })); });
    expect(node.textContent).toContain("未连接接入服务");
    expect(node.textContent).not.toContain("正在连接接入服务");
    expect(node.querySelector('[role="alert"]')?.textContent).toBe("电脑登记凭证已失效");
    await act(async () => { root.unmount(); }); node.remove();
  });

});
