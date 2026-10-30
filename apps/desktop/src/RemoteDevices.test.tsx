// @vitest-environment jsdom
import { act, createElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, describe, it, expect, vi } from "vitest";
const { invoke } = vi.hoisted(() => ({invoke:vi.fn()}));
vi.mock("@tauri-apps/api/core",()=>({invoke}));
vi.mock("qrcode",()=>({default:{toDataURL:vi.fn(async()=>"data:image/png;base64,fixture")}}));
import { RemoteDevices } from "./RemoteDevices";
describe("remote device pairing",()=>{
  beforeEach(() => {
    invoke.mockReset();
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
  });
  it("automatically shows one pairing code in StrictMode and cancels its identity on exit",async()=>{
    (globalThis as Record<string,unknown>).IS_REACT_ACT_ENVIRONMENT=true;
    const status = { configured: true, connected: true, service: "https://global.dog.codes", devices: [] };
    invoke.mockImplementation(async (_, { call }) => call.type === "invite"
      ? { ...status, invitation: { uri: "dogcodes://pair#automatic", expiresAt: Math.floor(Date.now() / 1000) + 300 } }
      : status);
    const node=document.createElement("div");document.body.append(node);const root=createRoot(node);
    await act(async()=>{root.render(createElement(StrictMode,null,createElement(RemoteDevices,{})));});
    expect(node.querySelector("img")?.alt).toBe("手机配对二维码");
    expect([...node.querySelectorAll("button")].some(button => /显示配对二维码|重新生成二维码/.test(button.textContent ?? ""))).toBe(false);
    expect(invoke.mock.calls.filter(([,args])=>args?.call?.type==="invite")).toHaveLength(1);
    const created=invoke.mock.calls.find(([,args])=>args?.call?.type==="invite")![1].call;
    expect(created.dialogId).toBeTruthy();
    expect(created.service).toBe("https://global.dog.codes");
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

  it("does not show an address field or send pairing requests to a retained local service", async () => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    invoke.mockClear();
    invoke.mockResolvedValue({ configured: true, connected: true, service: "http://192.168.1.161:8787", devices: [], invitation: { uri: "dogcodes://pair#old-local-service", expiresAt: Math.floor(Date.now() / 1000) + 300 } });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
    expect(node.querySelector("input")).toBeNull();
    expect(node.querySelector("img")).toBeNull();
    expect(node.textContent).not.toContain("Copy pairing link");
    expect(node.querySelector('[role="alert"]')?.textContent).toContain("background service has not updated");
    expect(node.querySelector(".remote-connection")?.classList.contains("disconnected")).toBe(true);
    expect([...node.querySelectorAll("button")].some(button => /Show pairing code|Generate new code/.test(button.textContent ?? ""))).toBe(false);
    expect(invoke.mock.calls.some(([, args]) => args?.call?.type === "invite")).toBe(false);
    await act(async () => { root.unmount(); }); node.remove();
  });

  it("does not report a failed service request as connected", async () => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    invoke.mockResolvedValue({ configured: true, connected: true, service: "https://global.dog.codes", devices: [], error: "Service unavailable" });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
    expect(node.querySelector(".remote-connection")?.textContent).toContain("Service disconnected");
    expect(node.querySelector('[role="alert"]')?.textContent).toContain("Service unavailable");
    await act(async () => { root.unmount(); }); node.remove();
  });

  it("clears a polling failure after the background service reconnects", async () => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    vi.useFakeTimers();
    let available = false;
    invoke.mockImplementation(async () => {
      if (!available) throw new Error("Background service restarting");
      return { configured: true, connected: true, service: "https://global.dog.codes", devices: [] };
    });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    try {
      await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
      expect(node.querySelector('[role="alert"]')?.textContent).toContain("Background service restarting");
      available = true;
      await act(async () => { await vi.advanceTimersByTimeAsync(1500); });
      expect(node.querySelector('[role="alert"]')).toBeNull();
      expect(node.querySelector(".remote-connection")?.textContent).toContain("Connected to service");
    } finally {
      await act(async () => { root.unmount(); }); node.remove(); vi.useRealTimers();
    }
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

  it("automatically replaces an expired code without regenerating it on every poll", async () => {
    (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
    vi.useFakeTimers();
    let invitation = { uri: "dogcodes://pair#first", expiresAt: Math.floor(Date.now() / 1000) + 3 };
    let created = 0;
    invoke.mockImplementation(async (_, { call }) => {
      if (call.type === "invite" && ++created > 1) {
        invitation = { uri: "dogcodes://pair#renewed", expiresAt: Math.floor(Date.now() / 1000) + 300 };
      }
      return { configured: true, connected: true, service: "https://global.dog.codes", devices: [], invitation };
    });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    try {
      await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
      expect(node.querySelector("img")).not.toBeNull();
      expect(node.textContent).toContain("Copy pairing link");
      await act(async () => { await vi.advanceTimersByTimeAsync(4500); });
      expect(created).toBe(2);
      expect(node.querySelector("img")).not.toBeNull();
      expect(node.textContent).toContain("Copy pairing link");
      expect(node.textContent).not.toContain("Code expired");
      expect(node.textContent).toContain(`Valid until ${new Intl.DateTimeFormat("en", { hour: "numeric", minute: "2-digit" }).format(invitation.expiresAt * 1000)}`);
      await act(async () => { await vi.advanceTimersByTimeAsync(4500); });
      expect(created).toBe(2);
    } finally {
      await act(async () => { root.unmount(); }); node.remove(); vi.useRealTimers();
    }
  });

  it("automatically recreates its invitation after a background handoff", async () => {
    vi.useFakeTimers();
    let invitation: { uri: string; expiresAt: number } | undefined;
    let created = 0;
    invoke.mockImplementation(async (_, { call }) => {
      if (call.type === "invite") invitation = { uri: `dogcodes://pair#${++created}`, expiresAt: Math.floor(Date.now() / 1000) + 300 };
      return { configured: true, connected: true, service: "https://global.dog.codes", devices: [], invitation };
    });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    try {
      await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
      expect(created).toBe(1);
      invitation = undefined;
      await act(async () => { await vi.advanceTimersByTimeAsync(1500); });
      expect(created).toBe(2);
      expect(node.querySelector("img")).not.toBeNull();
    } finally {
      await act(async () => { root.unmount(); }); node.remove(); vi.useRealTimers();
    }
  });

  it("does not duplicate an in-flight invitation and cancels it on exit", async () => {
    vi.useFakeTimers();
    const status = { configured: true, connected: true, service: "https://global.dog.codes", devices: [] };
    let finish!: (value: unknown) => void;
    invoke.mockImplementation(async (_, { call }) => call.type === "invite"
      ? new Promise(resolve => { finish = resolve; }) : status);
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    try {
      await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
      await act(async () => { await vi.advanceTimersByTimeAsync(6000); });
      const created = invoke.mock.calls.filter(([, args]) => args?.call?.type === "invite");
      expect(created).toHaveLength(1);
      expect(node.textContent).toContain("Processing");
      await act(async () => { root.unmount(); });
      expect(invoke).toHaveBeenCalledWith("core_remote_control", { call: { type: "cancel_invitation", dialogId: created[0][1].call.dialogId } });
      await act(async () => { finish({ ...status, invitation: { uri: "dogcodes://pair#late", expiresAt: Math.floor(Date.now() / 1000) + 300 } }); });
      expect(node.querySelector("img")).toBeNull();
    } finally {
      node.remove(); vi.useRealTimers();
    }
  });

  it("does not overwrite a generated code with an older status response", async () => {
    vi.useFakeTimers();
    const status = { configured: true, connected: true, service: "https://global.dog.codes", devices: [] };
    let finishFirstStatus!: (value: unknown) => void;
    let reads = 0;
    invoke.mockImplementation(async (_, { call }) => {
      if (call.type === "status" && ++reads === 1) return new Promise(resolve => { finishFirstStatus = resolve; });
      if (call.type === "invite") return { ...status, invitation: { uri: "dogcodes://pair#current", expiresAt: Math.floor(Date.now() / 1000) + 300 } };
      return status;
    });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    try {
      await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
      await act(async () => { await vi.advanceTimersByTimeAsync(1500); });
      expect(node.querySelector("img")).not.toBeNull();
      await act(async () => { finishFirstStatus(status); });
      expect(node.querySelector("img")).not.toBeNull();
    } finally {
      await act(async () => { root.unmount(); }); node.remove(); vi.useRealTimers();
    }
  });

  it("shows automatic generation failures without repeatedly sending the same request", async () => {
    vi.useFakeTimers();
    invoke.mockImplementation(async (_, { call }) => {
      if (call.type === "invite") throw new Error("Cannot create pairing invitation");
      return { configured: true, connected: true, service: "https://global.dog.codes", devices: [] };
    });
    const node = document.createElement("div"); document.body.append(node); const root = createRoot(node);
    try {
      await act(async () => { root.render(createElement(RemoteDevices, { lang: "en" })); });
      await act(async () => { await vi.advanceTimersByTimeAsync(6000); });
      expect(node.querySelector('[role="alert"]')?.textContent).toContain("Cannot create pairing invitation");
      expect(node.textContent).not.toContain("Processing");
      expect(invoke.mock.calls.filter(([, args]) => args?.call?.type === "invite")).toHaveLength(1);
    } finally {
      await act(async () => { root.unmount(); }); node.remove(); vi.useRealTimers();
    }
  });

});
