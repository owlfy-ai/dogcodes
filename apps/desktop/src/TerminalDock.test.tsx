// @vitest-environment jsdom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { TerminalDock, type TerminalApi } from "./TerminalDock";

import type { TerminalInfo, TerminalConnection } from "./terminalClient";
import { inputFixture, inputTick, testCrypto } from "./terminalInput.fixture";
let inputFixtures = new WeakMap<TerminalApi, ReturnType<typeof inputFixture>>();
const detach = vi.fn();
const info = (id = "existing", index = 0): TerminalInfo => ({ id, epoch: "epoch", projectId: "project", directoryIndex: index, taskId: null, commandId: null, label: "终端", cwd: "/project/web", command: null, status: "running", exitCode: null, error: null, outputEnd: 0, createdAt: 1 });
const connection = (id: string): TerminalConnection => ({ id, detach: () => detach(id) });
const terminals = vi.hoisted(() => [] as { input?: (data: string) => void; binary?: (data: string) => void; modes: { mouseTrackingMode: string }; write: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn>; options: Record<string, unknown> }[]);
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80; rows = 24; options = {}; modes = { mouseTrackingMode: "none" }; write = vi.fn(); dispose = vi.fn();
    constructor() { terminals.push(this); }
    open() {} loadAddon() {} focus() {}
    onData(callback: (data: string) => void) { (this as typeof terminals[number]).input = callback; return { dispose() {} }; }
    onBinary(callback: (data: string) => void) { (this as typeof terminals[number]).binary = callback; return { dispose() {} }; }
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
let root: Root, element: HTMLDivElement;
beforeEach(() => {
  terminals.length = 0; detach.mockClear(); inputFixtures = new WeakMap();
  vi.stubGlobal("crypto", testCrypto);
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("matchMedia", () => ({ addEventListener() {}, removeEventListener() {} }));
  element = document.createElement("div"); document.body.append(element); root = createRoot(element);
});
afterEach(async () => { await act(async () => root.unmount()); element.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });

function client(): TerminalApi {
  const fixture = inputFixture();
  const api: TerminalApi = {
    open: vi.fn(async (_, index, _cols, _rows, receive) => { receive({ kind: "state", info: info(`session-${index}`, index) }); return connection(`session-${index}`); }),
    attach: vi.fn(async (id, receive) => { receive({ kind: "state", info: info(id) }); return connection(id); }),
    list: vi.fn(async () => []), get: vi.fn(async id => info(id)),
    input: fixture.client().input, resize: vi.fn(async () => {}), close: vi.fn(async () => {}),
  };
  inputFixtures.set(api, fixture); return api;
}
const userInput = (element: HTMLElement) => element.querySelector(".terminal-emulator")!.dispatchEvent(new KeyboardEvent("keydown", { key: "x", bubbles: true }));
const directories = ["/project/web", "/project/server"];

it("starts one session per directory on demand and preserves both when hidden or switching tabs", async () => {
  const api = client();
  const render = (open: boolean) => act(async () => root.render(<TerminalDock projectId="project" directories={directories} open={open} onClose={() => {}} api={api} />));
  await render(false); expect(api.open).not.toHaveBeenCalled();
  await render(true);
  expect(api.open).toHaveBeenCalledTimes(2);
  expect(vi.mocked(api.open).mock.calls.map(call => call.slice(0, 2))).toEqual([["project", 0], ["project", 1]]);
  await act(async () => (element.querySelectorAll('[role="tab"]')[1] as HTMLButtonElement).click());
  expect(element.querySelectorAll('[role="tab"]')[1].getAttribute("aria-selected")).toBe("true");
  await render(false); await render(true);
  expect(api.open).toHaveBeenCalledTimes(2); expect(api.close).not.toHaveBeenCalled();
  await act(async () => root.render(null));
  expect(detach).toHaveBeenCalledWith("session-0"); expect(detach).toHaveBeenCalledWith("session-1"); expect(api.close).not.toHaveBeenCalled();
});

it("detaches sessions that finish opening after StrictMode cleanup or unmount", async () => {
  const api = client(), resolvers: ((value: TerminalConnection) => void)[] = [];
  vi.mocked(api.open).mockImplementation(() => new Promise(resolve => resolvers.push(resolve)));
  await act(async () => root.render(<StrictMode><TerminalDock projectId="project" directories={directories} open onClose={() => {}} api={api} /></StrictMode>));
  expect(api.open).toHaveBeenCalledTimes(directories.length);
  await act(async () => root.render(null));
  await act(async () => resolvers.forEach((resolve, index) => resolve(connection(`late-${index}`))));
  expect(detach).toHaveBeenCalledTimes(resolvers.length); expect(api.close).not.toHaveBeenCalled();
  expect(terminals.every(terminal => terminal.dispose.mock.calls.length === 1)).toBe(true);
});

it("sends UTF-8 and control keys in order and does not restart an exited shell", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project/web"]} open onClose={() => {}} api={api} />));
  await act(async () => { userInput(element); terminals[0].input?.("中文"); terminals[0].input?.("\x03"); await inputTick(); });
  expect(inputFixtures.get(api)!.writes.flat()).toEqual([...new TextEncoder().encode("中文"), 3]);
  const receive = vi.mocked(api.open).mock.calls[0][4];
  await act(async () => receive({ kind: "data", bytes: [65] }));
  expect(terminals[0].write).toHaveBeenCalledWith(new Uint8Array([65]));
  await act(async () => receive({ kind: "state", info: { ...info(), status: "exited", exitCode: 0 } }));
  expect(element.textContent).toContain("终端已退出");
  expect(element.querySelector(".terminal-status button")).toBeNull();
  expect(api.open).toHaveBeenCalledTimes(1);
  expect(api.close).not.toHaveBeenCalled();
});

it("adds and selects another independent terminal immediately for a single directory", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project/web"]} open onClose={() => {}} api={api} />));
  await act(async () => (element.querySelector('[aria-label="新建终端"]') as HTMLButtonElement).click());
  expect(element.querySelector('[role="menu"]')).toBeNull();
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(2);
  expect(element.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("web · 2");
  expect(api.open).toHaveBeenCalledTimes(2);
  expect(vi.mocked(api.open).mock.calls[1].slice(0, 2)).toEqual(["project", 0]);
  expect(api.close).not.toHaveBeenCalled();
});

it("waits for a directory choice and supports cancelling the menu without creating a session", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={directories} open onClose={() => {}} api={api} />));
  const add = element.querySelector('[aria-label="新建终端"]') as HTMLButtonElement;
  await act(async () => add.click());
  expect(api.open).toHaveBeenCalledTimes(2);
  expect(element.querySelectorAll('[role="menuitem"]')).toHaveLength(2);
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(element.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement).toBe(add);
  await act(async () => add.click());
  await act(async () => document.body.dispatchEvent(new Event("pointerdown", { bubbles: true })));
  expect(element.querySelector('[role="menu"]')).toBeNull();
  await act(async () => add.click());
  await act(async () => (element.querySelectorAll('[role="menuitem"]')[1] as HTMLButtonElement).click());
  expect(api.open).toHaveBeenCalledTimes(3);
  expect(vi.mocked(api.open).mock.calls[2].slice(0, 2)).toEqual(["project", 1]);
  expect(element.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("server · 2");
  expect(element.querySelector('[role="menu"]')).toBeNull();
  expect(api.close).not.toHaveBeenCalled();
});

it("hides only the requested output and selects an adjacent tab without stopping", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={directories} open onClose={() => {}} api={api} />));
  await act(async () => (element.querySelector('[aria-label="关闭终端 server"]') as HTMLButtonElement).click());
  expect(detach).toHaveBeenCalledExactlyOnceWith("session-1"); expect(api.close).not.toHaveBeenCalled();
  expect(element.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("web");
  await act(async () => (element.querySelector('[aria-label="新建终端"]') as HTMLButtonElement).click());
  await act(async () => [...element.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(button => button.textContent === "新建 server")!.click());
  await act(async () => (element.querySelector('[aria-label="关闭终端 server · 2"]') as HTMLButtonElement).click());
  expect(element.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("web");
  expect(terminals[0].dispose).not.toHaveBeenCalled();
});

it("allows closing the last tab and opening another without hiding the panel", async () => {
  const api = client(), hide = vi.fn();
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project/web"]} open onClose={hide} api={api} />));
  await act(async () => (element.querySelector('[aria-label="关闭终端 web"]') as HTMLButtonElement).click());
  expect(detach).toHaveBeenCalledExactlyOnceWith("session-0"); expect(api.close).not.toHaveBeenCalled();
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(0);
  expect(element.textContent).toContain("暂无终端");
  expect(hide).not.toHaveBeenCalled();
  await act(async () => (element.querySelector('[aria-label="新建终端"]') as HTMLButtonElement).click());
  await act(async () => [...element.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(button => button.textContent === "新建 web")!.click());
  expect(api.open).toHaveBeenCalledTimes(2);
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(1);
});

it("opens the existing directory chooser from the empty state without starting a shell early", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={directories} open onClose={() => {}} api={api} />));
  for (const name of ["web", "server"]) await act(async () => (element.querySelector(`[aria-label="关闭终端 ${name}"]`) as HTMLButtonElement).click());
  expect(element.querySelector(".terminal-empty h3")?.textContent).toBe("暂无终端");
  await act(async () => element.querySelector<HTMLButtonElement>(".terminal-empty .review-empty-action")!.click());
  expect(api.open).toHaveBeenCalledTimes(2);
  expect(element.querySelectorAll('[role="menuitem"]')).toHaveLength(4);
  await act(async () => (element.querySelectorAll('[role="menuitem"]')[3] as HTMLButtonElement).click());
  expect(api.open).toHaveBeenCalledTimes(3);
  expect(vi.mocked(api.open).mock.calls[2].slice(0, 2)).toEqual(["project", 1]);
  expect(element.querySelector(".terminal-empty")).toBeNull();
});

it("does not suggest creating a terminal without a project directory", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={[]} open onClose={() => {}} api={api} />));
  expect(element.querySelector(".terminal-empty h3")?.textContent).toBe("尚未关联项目目录");
  expect(element.querySelector(".terminal-empty button")).toBeNull();
  expect(api.open).not.toHaveBeenCalled();
});

it.each(["success", "failure"])("stops the startup animation when opening settles: %s", async outcome => {
  const api = client();
  let resolve!: (value: TerminalConnection) => void, reject!: (error: Error) => void;
  vi.mocked(api.open).mockImplementation(() => new Promise((yes, no) => { resolve = yes; reject = no; }));
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  expect(element.querySelector('.terminal-status .loading-dots')).not.toBeNull();
  await act(async () => {
    if (outcome === "success") { vi.mocked(api.open).mock.calls[0][4]({ kind: "state", info: info("opened") }); resolve(connection("opened")); }
    else reject(new Error("启动失败"));
  });
  expect(element.querySelector('.loading-status')).toBeNull();
  if (outcome === "failure") {
    expect(element.textContent).toContain("启动失败");
    expect(element.textContent).toContain("自动重新连接");
  } else expect(element.querySelector('.terminal-status')).toBeNull();
});

it("restores an existing background shell without opening another", async () => {
  const api = client(); vi.mocked(api.list).mockResolvedValue([info()]);
  await act(async () => root.render(<TerminalDock projectId="project" directories={directories} open onClose={() => {}} api={api} />));
  expect(api.attach).toHaveBeenCalledWith("existing", expect.any(Function));
  expect(api.open).not.toHaveBeenCalled();
  await act(async () => (element.querySelector('[aria-label="关闭终端 web"]') as HTMLButtonElement).click());
  expect(detach).toHaveBeenCalledWith("existing"); expect(api.close).not.toHaveBeenCalled();
  expect(element.querySelector('[aria-label="恢复后台终端"]')).toBeNull();
  await act(async () => (element.querySelector('[aria-label="新建终端"]') as HTMLButtonElement).click());
  await act(async () => (element.querySelector('[role="menuitem"]') as HTMLButtonElement).click());
  expect(api.attach).toHaveBeenCalledTimes(2); expect(api.open).not.toHaveBeenCalled();
  await act(async () => element.querySelector<HTMLButtonElement>('[aria-label="关闭当前终端"]')!.click());
  expect(api.close).toHaveBeenCalledExactlyOnceWith("existing", "epoch");
});

it("does not create shells when the background inventory cannot be checked", async () => {
  const api = client(); vi.mocked(api.list).mockRejectedValue(new Error("offline"));
  await act(async () => root.render(<TerminalDock projectId="project" directories={directories} open onClose={() => {}} api={api} />));
  expect(element.querySelector('[role="alert"]')?.textContent).toContain("无法查询后台终端");
  expect((element.querySelector('[aria-label="新建终端"]') as HTMLButtonElement).disabled).toBe(true);
  expect(api.open).not.toHaveBeenCalled();
});

it("automatically checks the original operation after a lost launch reply instead of creating a new one", async () => {
  vi.useFakeTimers();
  const api = client(); vi.mocked(api.open).mockRejectedValue(new Error("reply lost"));
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  const operationId = vi.mocked(api.open).mock.calls[0][5];
  expect(element.querySelector('.terminal-status button')).toBeNull();
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(api.attach).toHaveBeenCalledWith(operationId, expect.any(Function));
  expect(api.open).toHaveBeenCalledTimes(1);
  expect(api.close).not.toHaveBeenCalled();
});

it("unknown input pauses only input, preserves output and resumes only after explicit inspection", async () => {
  vi.useFakeTimers(); const api = client(), fixture = inputFixtures.get(api)!; fixture.notAdmitted = true;
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  await act(async () => { userInput(element); terminals[0].input?.("one\n"); await inputTick(); });
  expect(element.textContent).toContain("输入结果尚未确认"); expect(element.textContent).not.toContain("终端已退出");
  expect(terminals[0].options.disableStdin).toBe(true); expect(detach).not.toHaveBeenCalled();
  const receive = vi.mocked(api.open).mock.calls[0][4];
  await act(async () => receive({ kind: "data", bytes: [65] }));
  expect(terminals[0].write).toHaveBeenCalledWith(new Uint8Array([65]));
  await act(async () => receive({ kind: "state", info: info("session-0") }));
  expect(element.textContent).toContain("输入结果尚未确认"); expect(terminals[0].options.disableStdin).toBe(true);
  await act(async () => vi.advanceTimersByTimeAsync(1000)); expect(api.attach).not.toHaveBeenCalled();
  const button = (text: string) => [...element.querySelectorAll<HTMLButtonElement>(".terminal-input-recovery button")].find(button => button.textContent === text)!;
  const before = vi.mocked(api.input).mock.calls.length;
  await act(async () => { button("核实输入").click(); await inputTick(); });
  expect(vi.mocked(api.input).mock.calls.slice(before).map(call => call[1].type)).toEqual(["inspect"]);
  fixture.notAdmitted = false;
  await act(async () => { button("继续输入").click(); await inputTick(); });
  expect(terminals[0].options.disableStdin).toBe(false); expect(fixture.writes).toEqual([]);
  await act(async () => { terminals[0].input?.("new\n"); await inputTick(); });
  expect(new TextDecoder().decode(Uint8Array.from(fixture.writes.flat()))).toBe("new\n");
  expect(api.open).toHaveBeenCalledTimes(1); expect(api.close).not.toHaveBeenCalled();
});

it("viewing terminal protocol replies does not claim input; a keyboard gesture may claim it", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  await act(async () => { terminals[0].input?.("\x1b[1;1R"); await inputTick(); });
  expect(api.input).not.toHaveBeenCalled();
  await act(async () => { userInput(element); terminals[0].input?.("hello"); await inputTick(); });
  expect(new TextDecoder().decode(Uint8Array.from(inputFixtures.get(api)!.writes.flat()))).toBe("hello");
});

it("an ordinary focus click stays passive but a TUI mouse report may acquire input", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  const emulator = element.querySelector(".terminal-emulator")!;
  await act(async () => { emulator.dispatchEvent(new Event("pointerdown", { bubbles: true })); terminals[0].input?.("\x1b[1;1R"); await inputTick(); });
  expect(api.input).not.toHaveBeenCalled();
  await act(async () => { terminals[0].modes.mouseTrackingMode = "vt200"; emulator.dispatchEvent(new Event("pointerdown", { bubbles: true })); terminals[0].binary?.("\x1b[M !!"); await inputTick(); });
  expect(inputFixtures.get(api)!.writes.flat()).toEqual([27,91,77,32,33,33]);
});

it("output reattachment retains unresolved input, and read-only proof enables input without replay", async () => {
  vi.useFakeTimers(); const api = client(), fixture = inputFixtures.get(api)!; let finish!: () => void;
  fixture.gate = new Promise<void>(resolve => { finish = resolve; });
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  await act(async () => { userInput(element); terminals[0].input?.("one"); await inputTick(); });
  const receive = vi.mocked(api.open).mock.calls[0][4];
  await act(async () => receive({ kind: "disconnected", error: "offline" }));
  await act(async () => vi.advanceTimersByTimeAsync(1000)); expect(api.attach).toHaveBeenCalledTimes(1);
  await act(async () => { finish(); await inputTick(); });
  expect(terminals.at(-1)!.options.disableStdin).toBe(true);
  const verify = [...element.querySelectorAll<HTMLButtonElement>(".terminal-input-recovery button")].find(button => button.textContent === "核实输入")!;
  await act(async () => { verify.click(); await inputTick(); });
  expect(terminals.at(-1)!.options.disableStdin).toBe(false); expect(fixture.writes.length).toBe(1);
  expect(api.open).toHaveBeenCalledTimes(1); expect(api.close).not.toHaveBeenCalled();
});

it("replacing an API wrapper preserves the same terminal's unknown input identity", async () => {
  const api = client(), fixture = inputFixtures.get(api)!; fixture.notAdmitted = true;
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  await act(async () => { userInput(element); terminals[0].input?.("old"); await inputTick(); });
  expect(element.querySelector(".terminal-input-recovery")).not.toBeNull();
  const next = { ...api };
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={next} />));
  expect(terminals.at(-1)!.options.disableStdin).toBe(true);
  const before = vi.mocked(api.input).mock.calls.length;
  const verify = [...element.querySelectorAll<HTMLButtonElement>(".terminal-input-recovery button")].find(button => button.textContent === "核实输入")!;
  await act(async () => { verify.click(); await inputTick(); });
  expect(vi.mocked(api.input).mock.calls.slice(before).map(call => call[1].type)).toEqual(["inspect"]);
  expect(vi.mocked(api.input).mock.calls.filter(call => call[1].type === "write").length).toBe(1);
});

it("shows a friendly startup error instead of the raw exception, without a manual recovery button", async () => {
  const api = client();
  vi.mocked(api.open).mockRejectedValue(new TypeError("Cannot read properties of undefined (reading 'invoke')"));
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  const alert = element.querySelector('.terminal-status .inline-error[role="alert"]');
  expect(alert?.textContent).toContain("无法连接终端");
  expect(element.textContent).not.toContain("Cannot read properties");
  expect(alert?.querySelector("button.inline-error-retry")).toBeNull();
});


it("reopens a newly created terminal by its remembered resource identity", async () => {
  const api = client();
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project/web"]} open onClose={() => {}} api={api} />));
  await act(async () => element.querySelector<HTMLButtonElement>('[aria-label="关闭终端 web"]')!.click());
  await act(async () => element.querySelector<HTMLButtonElement>('[aria-label="新建终端"]')!.click());
  await act(async () => element.querySelector<HTMLButtonElement>('[role="menuitem"]')!.click());
  expect(api.attach).toHaveBeenCalledWith("session-0", expect.any(Function));
  expect(api.open).toHaveBeenCalledTimes(1);
  expect(api.close).not.toHaveBeenCalled();
});

it("backs off reconnects to the same identity and pauses retries while hidden", async () => {
  vi.useFakeTimers();
  const api = client();
  vi.mocked(api.list).mockResolvedValue([info()]);
  vi.mocked(api.attach).mockRejectedValue(new Error("offline"));
  const render = (open: boolean) => act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open={open} onClose={() => {}} api={api} />));
  await render(true);
  expect(api.attach).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(api.attach).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(1999));
  expect(api.attach).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(api.attach).toHaveBeenCalledTimes(3);
  await render(false);
  await act(async () => vi.advanceTimersByTimeAsync(60000));
  expect(api.attach).toHaveBeenCalledTimes(3);
  await render(true);
  expect(api.attach).toHaveBeenCalledTimes(4);
  expect(vi.mocked(api.attach).mock.calls.every(call => call[0] === "existing")).toBe(true);
  expect(api.open).not.toHaveBeenCalled();
});

it("does not relaunch when a lost launch reply cannot be resolved by its original identity", async () => {
  vi.useFakeTimers();
  const api = client();
  vi.mocked(api.open).mockRejectedValue(new Error("reply lost"));
  vi.mocked(api.attach).mockRejectedValue(new Error("not found"));
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  const id = vi.mocked(api.open).mock.calls[0][5];
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(vi.mocked(api.attach).mock.calls.map(call => call[0])).toEqual([id, id]);
  expect(api.open).toHaveBeenCalledTimes(1);
  expect(api.close).not.toHaveBeenCalled();
});

it("discovers newly available background terminals on reopening without duplicating known tabs", async () => {
  const api = client();
  vi.mocked(api.list).mockResolvedValue([info()]);
  const render = (open: boolean) => act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open={open} onClose={() => {}} api={api} />));
  await render(true);
  await render(false);
  vi.mocked(api.list).mockResolvedValue([info(), info("another")]);
  await render(true);
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(2);
  expect(api.attach).toHaveBeenCalledTimes(2);
  expect(api.attach).toHaveBeenLastCalledWith("another", expect.any(Function));
  expect(api.open).not.toHaveBeenCalled();
});


it("does not retry a connection while the previous attach is still pending", async () => {
  vi.useFakeTimers();
  const api = client();
  vi.mocked(api.open).mockRejectedValue(new Error("reply lost"));
  vi.mocked(api.attach).mockImplementation(() => new Promise(() => {}));
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(api.attach).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(60000));
  expect(api.attach).toHaveBeenCalledTimes(1);
  expect(api.open).toHaveBeenCalledTimes(1);
});

it("shows an unknown process without reconnecting or restarting it", async () => {
  vi.useFakeTimers();
  const api = client();
  vi.mocked(api.list).mockResolvedValue([{ ...info(), status: "unknown", error: "后台已重启，原进程状态未知" }]);
  vi.mocked(api.attach).mockImplementation(async (id, receive) => {
    receive({ kind: "state", info: { ...info(id), status: "unknown", error: "后台已重启，原进程状态未知" } });
    return connection(id);
  });
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  await act(async () => vi.advanceTimersByTimeAsync(60000));
  expect(element.textContent).toContain("原进程状态未知");
  expect(api.attach).toHaveBeenCalledTimes(1);
  expect(api.open).not.toHaveBeenCalled();
});


it("closes only the selected terminal through the toolbar cross", async () => {
  const api = client(), hide = vi.fn();
  await act(async () => root.render(<TerminalDock projectId="project" directories={directories} open onClose={hide} api={api} />));
  await act(async () => element.querySelectorAll<HTMLButtonElement>('[role="tab"]')[1].click());
  expect(element.querySelector(".terminal-controls")).toBeNull();
  expect(element.querySelector('[aria-label="收起终端"]')).toBeNull();
  await act(async () => element.querySelector<HTMLButtonElement>('[aria-label="关闭当前终端"]')!.click());
  expect(api.close).toHaveBeenCalledExactlyOnceWith("session-1", "epoch");
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(1);
  expect(element.querySelector('[role="tab"]')?.textContent).toBe("web");
  expect(terminals[0].dispose).not.toHaveBeenCalled();
  expect(hide).not.toHaveBeenCalled();
});

it("keeps the current terminal visible on close failure and allows retry", async () => {
  const api = client(), hide = vi.fn();
  vi.mocked(api.close).mockRejectedValueOnce(new Error("关闭失败"));
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={hide} api={api} />));
  await act(async () => element.querySelector<HTMLButtonElement>('[aria-label="关闭当前终端"]')!.click());
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(1);
  expect(element.querySelector('[role="alert"]')?.textContent).toContain("关闭失败");
  expect(hide).not.toHaveBeenCalled();
  expect(detach).not.toHaveBeenCalled();
  await act(async () => element.querySelector<HTMLButtonElement>('[aria-label="关闭当前终端"]')!.click());
  expect(api.close).toHaveBeenCalledTimes(2);
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(0);
  expect(hide).toHaveBeenCalledTimes(1);
});

it("waits for close to finish and does not send duplicate close requests", async () => {
  const api = client();
  let finish!: () => void;
  vi.mocked(api.close).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  await act(async () => root.render(<TerminalDock projectId="project" directories={["/project"]} open onClose={() => {}} api={api} />));
  const close = element.querySelector<HTMLButtonElement>('[aria-label="关闭当前终端"]')!;
  await act(async () => { close.click(); close.click(); });
  expect(api.close).toHaveBeenCalledTimes(1);
  expect(close.disabled).toBe(true);
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(1);
  await act(async () => finish());
  expect(element.querySelectorAll('[role="tab"]')).toHaveLength(0);
});
