// @vitest-environment jsdom
import { act, createElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { describe, it, expect, vi } from "vitest";
const { invoke } = vi.hoisted(() => ({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke}));
vi.mock("qrcode",()=>({default:{toDataURL:vi.fn(async()=>"data:image/png;base64,fixture")}}));
import { RemoteDevices } from "./RemoteDevices";
describe("remote device pairing",()=>{
  it("cancels the same invitation identity when leaving the page",async()=>{
    (globalThis as Record<string,unknown>).IS_REACT_ACT_ENVIRONMENT=true;
    invoke.mockResolvedValue({configured:false,connected:false,devices:[]});
    const node=document.createElement("div");document.body.append(node);const root=createRoot(node);
    await act(async()=>{root.render(createElement(StrictMode,null,createElement(RemoteDevices,{})));});
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
    await act(async () => { root.render(createElement(RemoteDevices, {})); });
    expect(node.textContent).toContain("未连接接入服务");
    expect(node.textContent).not.toContain("正在连接接入服务");
    expect(node.querySelector('[role="alert"]')?.textContent).toContain("电脑登记凭证已失效");
    await act(async () => { root.unmount(); }); node.remove();
  });

  it("updates language without replacing the invitation and formats its expiry for that language", async () => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    invoke.mockClear();
    const expiresAt = Math.floor(Date.now() / 1000) + 300;
    invoke.mockResolvedValue({ configured: true, connected: true, devices: [], invitation: { uri: "dogcodes://pair#fixture", expiresAt } });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    await act(async () => { root.render(createElement(RemoteDevices, { lang: "zh-CN" })); });
    expect(node.querySelector(".remote-page")?.getAttribute("aria-label")).toBe("连接手机");
    expect(node.querySelector("img")?.alt).toBe("手机配对二维码");
    const calls = invoke.mock.calls.length;
    await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
    expect(node.querySelector(".remote-page")?.getAttribute("aria-label")).toBe("Connect phone");
    expect(node.querySelector("img")?.alt).toBe("Phone pairing QR code");
    expect(node.textContent).toContain(`Valid until ${new Intl.DateTimeFormat("en", { hour: "numeric", minute: "2-digit" }).format(expiresAt * 1000)}`);
    expect(node.textContent).not.toMatch(/[\u3400-\u9fff]/);
    expect(invoke.mock.calls.length).toBe(calls);
    await act(async () => { root.unmount(); }); node.remove();
  });

  it("removes an expired QR code and copy action while keeping regeneration available", async () => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    vi.useFakeTimers();
    const expiresAt = Math.floor(Date.now() / 1000) + 3;
    invoke.mockResolvedValue({ configured: true, connected: true, devices: [], invitation: { uri: "dogcodes://pair#fixture", expiresAt } });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    try {
      await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
      expect(node.querySelector("img")).not.toBeNull();
      expect(node.textContent).toContain("Copy pairing link");
      await act(async () => { await vi.advanceTimersByTimeAsync(4500); });
      expect(node.querySelector("img")).toBeNull();
      expect(node.textContent).not.toContain("Copy pairing link");
      expect(node.textContent).toContain("Code expired");
      expect([...node.querySelectorAll("button")].find(button => button.textContent === "Generate new code")?.disabled).toBe(false);
    } finally {
      await act(async () => { root.unmount(); }); node.remove(); vi.useRealTimers();
    }
  });

});
