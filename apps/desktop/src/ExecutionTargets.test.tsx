// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ExecutionTargets, HostProjectDialog, TargetConnectionDialog, TargetOfflineBanner, TargetOfflinePanel } from "./ExecutionTargets";
import { I18nProvider } from "./i18n";
import type { TerminalApi, TerminalInfo } from "./terminalClient";
import { connectionFromForm, joinDirectory, parentDirectory, stepStates, targetRoute, type TargetApi, type TargetStatus } from "./executionTargetClient";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn(() => false) }));

vi.mock("./TerminalDock", () => ({
  TerminalPane: ({ operationId, onStatus, visible }: { operationId: string; onStatus?: (status: string, epoch: string) => void; visible: boolean }) =>
    <div data-testid="shell" data-operation={operationId} data-visible={visible}><button onClick={() => onStatus?.("running", "e1")}>shell started</button><button onClick={() => onStatus?.("unknown", "e1")}>shell lost</button></div>,
}));

const idle: TargetStatus = { scanning: false, agents: [], phase: "idle", step: null, log: [], prompt: null, info: null, error: null, closedReason: null, identityChanged: false };
const emptyForm = { destination: "", port: "", user: "", auth: "system" as const, identityFile: "", installDirectory: "" };

function fakeApi(status: TargetStatus, overrides: Partial<TargetApi> = {}): TargetApi {
  return {
    list: vi.fn(async () => []),
    status: vi.fn(async () => status),
    save: vi.fn(async () => "tgt_new"),
    remove: vi.fn(async () => {}),
    connect: vi.fn(async () => {}),
    disconnect: vi.fn(async () => {}),
    rescan: vi.fn(async () => {}),
    release: vi.fn(async () => {}),
    answer: vi.fn(async () => {}),
    configHosts: vi.fn(async () => []),
    resolveHost: vi.fn(async () => null),
    browse: vi.fn(async (_, path) => ({ path: path ?? "/home/dev", directories: ["app"] })),
    createProject: vi.fn(async () => "prj_new"),
    ...overrides,
  };
}

let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

const entry = {
  target: { targetId: "tgt_one", displayName: "Build", connection: { destination: "build", auth: "password" as const } },
  status: idle,
  agents: [],
};
const renderDialog = async (api: TargetApi, initialProgress = false, current: typeof entry | null = entry) => {
  await act(async () => {
    root.render(<I18nProvider language="en"><TargetConnectionDialog api={api} entry={current} initialProgress={initialProgress} onClose={() => {}} onChanged={() => {}} /></I18nProvider>);
  });
};
const setValue = async (input: HTMLInputElement, value: string) => {
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
};
const button = (text: string) => [...document.querySelectorAll("button")].find(item => item.textContent === text);

describe("connection values", () => {
  it("saves a proxy only when one is chosen", () => {
    const base = { ...emptyForm, destination: "a" };
    expect(connectionFromForm({ ...base, proxyMode: "none", proxyHost: "ignored", proxyPort: "1" })).not.toHaveProperty("proxy");
    expect(connectionFromForm({ ...base, proxyMode: "system" }).proxy).toEqual({ mode: "system" });
    expect(connectionFromForm({ ...base, proxyMode: "custom", proxyKind: "http", proxyHost: " 127.0.0.1 ", proxyPort: "7890" }).proxy)
      .toEqual({ mode: "custom", kind: "http", host: "127.0.0.1", port: 7890 });
    // Left for the service to refuse, rather than quietly connecting without a proxy.
    expect(connectionFromForm({ ...base, proxyMode: "custom", proxyHost: "", proxyPort: "" }).proxy)
      .toEqual({ mode: "custom", kind: "socks5", host: "", port: 0 });
  });

  it("leaves empty fields out so ssh resolves them from its config", () => {
    expect(connectionFromForm({ ...emptyForm, destination: " gpu-box " })).toEqual({ destination: "gpu-box", auth: "system" });
  });
  it("keeps an identity file only for private key authentication", () => {
    expect(connectionFromForm({ ...emptyForm, destination: "a", identityFile: "~/k" })).not.toHaveProperty("identityFile");
    expect(connectionFromForm({ ...emptyForm, destination: "a", auth: "private_key", identityFile: "~/k" }).identityFile).toBe("~/k");
  });
  it("shows the route the way people write it", () => {
    expect(targetRoute({ destination: "h", user: "u", port: 2222, auth: "system" })).toBe("u@h:2222");
    expect(targetRoute({ destination: "alias", auth: "system" })).toBe("alias");
  });
  it("marks the step an attempt stopped at", () => {
    const states = stepStates({ phase: "failed", step: { step: "inspecting" } });
    expect(states.map(item => item.state)).toEqual(["done", "failed", "pending", "pending"]);
  });
  it("treats skipped installation as passed once the shim is starting", () => {
    const states = stepStates({ phase: "connecting", step: { step: "starting" } });
    expect(states.map(item => item.state)).toEqual(["done", "done", "done", "active"]);
    expect(stepStates({ phase: "connected", step: { step: "ready" } }).every(item => item.state === "done")).toBe(true);
  });
});

describe("connection form", () => {
  it("offers system default authentication first for a new connection", async () => {
    await renderDialog(fakeApi(idle), false, null);
    expect(document.querySelector('[role="radio"][aria-checked="true"]')?.textContent).toBe("System default");
    expect(document.querySelector('input[type="password"]')).toBeNull();
  });

  const submit = () => act(async () => { document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });

  it("connects a new host to find its agents, and lets go once closed", async () => {
    const api = fakeApi({ ...idle, phase: "connected", step: { step: "ready" }, scanning: true });
    const onClose = vi.fn();
    await act(async () => { root.render(<I18nProvider language="en"><TargetConnectionDialog api={api} entry={null} onClose={onClose} onChanged={() => {}} /></I18nProvider>); });
    expect(api.connect).not.toHaveBeenCalled();
    await setValue(document.querySelector<HTMLInputElement>("input[required]")!, "gpu");
    await submit();
    expect(api.connect).toHaveBeenCalledWith("tgt_new", null);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    const done = [...document.querySelectorAll("footer button")].find(item => item.textContent === "Done") as HTMLButtonElement;
    expect(done.disabled).toBe(true);
    await act(async () => { document.querySelector<HTMLButtonElement>('[aria-label="Close"]')!.click(); });
    expect(api.release).toHaveBeenCalledWith("tgt_new");
    expect(onClose).toHaveBeenCalled();
  });

  it("only saves a host: it keeps the secret and connects to nothing", async () => {
    const api = fakeApi(idle);
    const onClose = vi.fn();
    await act(async () => { root.render(<I18nProvider language="en"><TargetConnectionDialog api={api} entry={entry} onClose={onClose} onChanged={() => {}} /></I18nProvider>); });
    await setValue(document.querySelector('input[type="password"]')!, "s3cret");
    await submit();
    expect(JSON.stringify(vi.mocked(api.save).mock.calls[0]![2])).not.toContain("s3cret");
    expect(vi.mocked(api.save).mock.calls[0]![3]).toBe("s3cret");
    expect(api.connect).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("leaves open sessions alone when only the alias changed", async () => {
    const api = fakeApi(idle);
    await renderDialog(api);
    await setValue(document.querySelector<HTMLInputElement>(".target-form > label input")!, "Renamed");
    await submit();
    expect(vi.mocked(api.save).mock.calls[0]![1]).toBe("Renamed");
    expect(api.disconnect).not.toHaveBeenCalled();
  });

  it("ends the old connection when the route changed", async () => {
    const api = fakeApi(idle);
    await renderDialog(api);
    await setValue(document.querySelector<HTMLInputElement>("input[required]")!, "other");
    await submit();
    expect(api.disconnect).toHaveBeenCalledWith("tgt_new");
  });

  it("imports an SSH config host as an alias, leaving its values to ssh", async () => {
    const api = fakeApi(idle, { configHosts: vi.fn(async () => ["gpu"]), resolveHost: vi.fn(async () => ({ host: "10.0.0.2", user: "dev", port: 22 })) });
    await renderDialog(api, false, null);
    const select = document.querySelector("select")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(select, "gpu");
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(document.querySelector<HTMLInputElement>("input[required]")!.value).toBe("gpu");
    expect(document.querySelector(".target-route")?.textContent).toBe("dev@10.0.0.2:22");
    await act(async () => { document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    expect(vi.mocked(api.save).mock.calls[0]![2]).toEqual({ destination: "gpu", auth: "system" });
  });
});

describe("connection progress", () => {
  it("never saves a host key answer as a secret", async () => {
    const api = fakeApi({ ...idle, phase: "connecting", step: { step: "connecting" }, prompt: { id: "p1", kind: "host_key", text: "fingerprint SHA256:abc" } });
    await renderDialog(api, true);
    expect(document.body.textContent).toContain("SHA256:abc");
    await act(async () => { button("Trust and continue")!.click(); });
    expect(api.answer).toHaveBeenCalledWith("tgt_one", "p1", "yes", false);
  });

  it("cancels a question with no answer rather than an empty one", async () => {
    const api = fakeApi({ ...idle, phase: "connecting", prompt: { id: "p2", kind: "password", text: "password:" } });
    await renderDialog(api, true);
    await act(async () => { button("Cancel")!.click(); });
    expect(api.answer).toHaveBeenCalledWith("tgt_one", "p2", null, false);
  });

  it("offers no way to ignore a changed host key", async () => {
    await renderDialog(fakeApi({ ...idle, phase: "failed", error: { kind: "host_key_changed", detail: "changed" } }), true);
    expect(document.querySelector('[role="alert"]')?.textContent).toContain("identity has changed");
    expect(button("Trust and continue")).toBeUndefined();
  });

  it("warns when a saved route reaches a different machine", async () => {
    await renderDialog(fakeApi({ ...idle, phase: "connected", identityChanged: true, info: { protocol: 1, version: "1", os: "linux", arch: "x86_64", home: "/h", shell: null, machineIdentity: "m" } }), true);
    expect(document.body.textContent).toContain("different machine");
  });
});

describe("remote projects", () => {
  const connected: TargetStatus = { ...idle, phase: "connected", step: { step: "ready" },
    info: { protocol: 2, version: "1", os: "linux", arch: "x86_64", home: "/home/dev", shell: null, machineIdentity: "m" } };

  it("lists the agents found and explains each state", async () => {
    await act(async () => { root.render(<I18nProvider language="en"><TargetConnectionDialog api={fakeApi({ ...connected, agents: [
      { providerId: "kimi", displayName: "Kimi Code", executable: "/k", version: "2.5.0", status: "ready", diagnostic: null },
      { providerId: "codex", displayName: "Codex", executable: "/c", version: "1", status: "not_configured", diagnostic: "x" },
    ] })} entry={entry} initialProgress onClose={() => {}} onChanged={() => {}} /></I18nProvider>); });
    expect(document.body.textContent).toContain("Kimi Code");
    expect(document.body.textContent).toContain("Sign in on the server");
  });

  it("waits for the scan before offering to open a project", async () => {
    await act(async () => { root.render(<I18nProvider language="en"><TargetConnectionDialog api={fakeApi({ ...connected, scanning: true })} entry={entry} initialProgress onClose={() => {}} onChanged={() => {}} onProjectReady={() => {}} /></I18nProvider>); });
    expect(button("Open a project")!.disabled).toBe(true);
  });

  it("browses from the home directory and creates the project where it stops", async () => {
    const api = fakeApi(connected);
    const ready = vi.fn();
    await act(async () => { root.render(<I18nProvider language="en"><TargetConnectionDialog api={api} entry={entry} initialProgress initialBrowsing onClose={() => {}} onChanged={() => {}} onProjectReady={ready} /></I18nProvider>); });
    expect(api.browse).toHaveBeenCalledWith("tgt_one", null);
    await act(async () => { button("app")!.click(); });
    expect(api.browse).toHaveBeenLastCalledWith("tgt_one", "/home/dev/app");
    await act(async () => { button("Open here")!.click(); });
    expect(api.createProject).toHaveBeenCalledWith("tgt_one", "/home/dev/app", "");
    expect(ready).toHaveBeenCalledWith("prj_new");
  });

  const banner = async (api: TargetApi) => {
    await act(async () => { root.render(<I18nProvider language="en"><TargetOfflineBanner api={api} targetId="tgt_one" /></I18nProvider>); });
  };
  const flush = async () => { await act(async () => { await new Promise(resolve => setTimeout(resolve, 450)); }); };

  const autoBanner = async (api: TargetApi) => {
    await act(async () => { root.render(<I18nProvider language="en"><TargetOfflineBanner api={api} targetId="tgt_one" autoConnect /></I18nProvider>); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)); });
  };

  it("connects by itself when a project on the host is opened", async () => {
    const api = fakeApi(idle);
    await autoBanner(api);
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
  });

  it("doesn't connect again while connected", async () => {
    const api = fakeApi(connected);
    await autoBanner(api);
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("asks for a password when connecting by itself needs one", async () => {
    const api = fakeApi(idle, { list: vi.fn(async () => [entry]) });
    await autoBanner(api);
    vi.mocked(api.status).mockResolvedValue({ ...idle, phase: "connecting", prompt: { id: "p", kind: "password", text: "Password for build" } });
    await flush();
    await flush();
    expect(document.body.textContent).toContain("Password for build");
  });

  it("states a failure in the banner, with no dialog, and doesn't retry what it can't fix", async () => {
    const api = fakeApi(idle, { list: vi.fn(async () => [entry]) });
    await autoBanner(api);
    vi.mocked(api.connect).mockClear();
    vi.mocked(api.status).mockResolvedValue({ ...idle, phase: "failed", error: { kind: "auth_failed", detail: "denied" } });
    await flush();
    await flush();
    expect(document.querySelector(".target-offline")?.textContent).toContain("Sign-in failed");
    expect(document.querySelector("[role=dialog]")).toBeNull();
    expect(api.connect).not.toHaveBeenCalled();
  });

  it("connects again after losing the connection", async () => {
    vi.useFakeTimers();
    try {
      const api = fakeApi({ ...idle, phase: "disconnected" });
      await act(async () => { root.render(<I18nProvider language="en"><TargetOfflineBanner api={api} targetId="tgt_one" autoConnect /></I18nProvider>); });
      await act(async () => { await vi.advanceTimersByTimeAsync(10); });
      expect(api.connect).not.toHaveBeenCalled();
      await act(async () => { await vi.advanceTimersByTimeAsync(3000); });
      expect(api.connect).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("retries a network failure", async () => {
    vi.useFakeTimers();
    try {
      const api = fakeApi({ ...idle, phase: "failed", error: { kind: "host_unreachable", detail: "x" } });
      await act(async () => { root.render(<I18nProvider language="en"><TargetOfflineBanner api={api} targetId="tgt_one" autoConnect /></I18nProvider>); });
      await act(async () => { await vi.advanceTimersByTimeAsync(10); });
      expect(api.connect).not.toHaveBeenCalled();
      await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
      expect(api.connect).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  const hostDialog = async (api: TargetApi, current: typeof entry | null, onClose = vi.fn(), ready = vi.fn()) => {
    await act(async () => { root.render(<I18nProvider language="en"><HostProjectDialog api={api} entry={current} onClose={onClose} onProjectReady={ready} /></I18nProvider>); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)); });
  };
  const withAgents = { ...connected, agents: [{ providerId: "kimi", displayName: "Kimi", executable: "/k", version: "1", status: "ready" as const, diagnostic: null }] };

  it("connects to a chosen host and goes on to its folders", async () => {
    const api = fakeApi({ ...idle, phase: "connecting", step: { step: "connecting" } });
    await hostDialog(api, entry);
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
    expect(document.querySelector(".target-steps")).not.toBeNull();
    vi.mocked(api.status).mockResolvedValue(withAgents);
    await flush();
    expect(document.querySelector(".target-browser")).not.toBeNull();
    expect(api.browse).toHaveBeenCalledWith("tgt_one", null);
  });

  it("goes straight to the folders of a connected host", async () => {
    const api = fakeApi(withAgents);
    await hostDialog(api, { ...entry, status: connected });
    expect(api.connect).not.toHaveBeenCalled();
    expect(document.querySelector(".target-browser")).not.toBeNull();
  });

  it("stops to explain a host without agents, and a different machine", async () => {
    const none = fakeApi(connected);
    await hostDialog(none, entry);
    await flush();
    expect(document.body.textContent).toContain("No supported agent");
    expect(document.querySelector(".target-browser")).toBeNull();
    await act(async () => root.unmount());
    root = createRoot(container);
    await hostDialog(fakeApi({ ...withAgents, identityChanged: true }), entry);
    await flush();
    expect(document.body.textContent).toContain("different machine");
    expect(document.querySelector(".target-browser")).toBeNull();
  });

  it("connects a new host once saved, then goes on to its folders", async () => {
    const api = fakeApi(withAgents);
    await hostDialog(api, null);
    expect(api.connect).not.toHaveBeenCalled();
    await setValue(document.querySelector<HTMLInputElement>("input[required]")!, "gpu");
    await act(async () => { document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    expect(api.connect).toHaveBeenCalledWith("tgt_new", null);
    await flush();
    expect(document.querySelector(".target-browser")).not.toBeNull();
  });

  it("releases the connection if no project came of it, and keeps it if one did", async () => {
    const api = fakeApi(connected);
    await hostDialog(api, entry);
    await act(async () => { button("Done")!.click(); });
    expect(api.release).toHaveBeenCalledWith("tgt_one");
    await act(async () => root.unmount());
    root = createRoot(container);
    const kept = fakeApi(withAgents);
    await hostDialog(kept, { ...entry, status: connected });
    await act(async () => { button("Open here")!.click(); });
    expect(kept.createProject).toHaveBeenCalled();
    expect(kept.release).not.toHaveBeenCalled();
  });

  it("reconnects from the banner without asking anything when nobody is needed", async () => {
    const api = fakeApi(idle);
    await banner(api);
    expect(document.body.textContent).toContain("not connected");
    vi.mocked(api.status).mockResolvedValue(connected);
    await act(async () => { button("Reconnect")!.click(); });
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
    await flush();
    expect(document.body.textContent).toBe("");
    expect(document.querySelector("[role=dialog]")).toBeNull();
  });

  it("shows the connection dialog in place when a password is needed", async () => {
    const api = fakeApi(idle, { list: vi.fn(async () => [entry]) });
    await banner(api);
    vi.mocked(api.status).mockResolvedValue({ ...idle, phase: "connecting", prompt: { id: "p", kind: "password", text: "Password for build" } });
    await act(async () => { button("Reconnect")!.click(); });
    await flush();
    expect(document.body.textContent).toContain("Password for build");
    expect(button("Reconnecting…")!.disabled).toBe(true);
  });

  it("does not take the last attempt's failure for this one", async () => {
    const failed = { ...idle, phase: "failed" as const, error: { kind: "auth_failed" as const, detail: "denied" } };
    const api = fakeApi(failed, { list: vi.fn(async () => [entry]) });
    await banner(api);
    expect(api.list).not.toHaveBeenCalled();
    vi.mocked(api.status).mockResolvedValue({ ...idle, phase: "connecting" });
    await act(async () => { button("Reconnect")!.click(); });
    await flush();
    expect(api.list).not.toHaveBeenCalled();
    vi.mocked(api.status).mockResolvedValue(failed);
    await flush();
    expect(api.list).toHaveBeenCalled();
  });

  const both = async (api: TargetApi) => {
    await act(async () => { root.render(<I18nProvider language="en">
      <TargetOfflineBanner api={api} targetId="tgt_one" />
      <TargetOfflinePanel api={api} targetId="tgt_one" visible onClose={() => {}}><p>panel content</p></TargetOfflinePanel>
    </I18nProvider>); });
  };

  it("hides the panel's content until the server is connected", async () => {
    await both(fakeApi(idle));
    expect(document.body.textContent).not.toContain("panel content");
    expect(document.querySelector("aside .review-empty-action")?.textContent).toBe("Reconnect");
  });

  it.each([".target-offline button", "aside .review-empty-action"])("connecting from %s updates both views at once", async selector => {
    const api = fakeApi(idle);
    await both(api);
    vi.mocked(api.status).mockResolvedValue(connected);
    await act(async () => { document.querySelector<HTMLButtonElement>(selector)!.click(); });
    expect(api.connect).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".target-offline")).toBeNull();
    expect(document.body.textContent).toBe("panel content");
  });

  it("asks for a password once, in the view that was clicked", async () => {
    const api = fakeApi(idle, { list: vi.fn(async () => [entry]) });
    await both(api);
    vi.mocked(api.status).mockResolvedValue({ ...idle, phase: "connecting", prompt: { id: "p", kind: "password", text: "Password for build" } });
    await act(async () => { document.querySelector<HTMLButtonElement>("aside .review-empty-action")!.click(); });
    await flush();
    await flush();
    expect(document.querySelectorAll("[role=dialog]").length).toBe(1);
  });

  it("stays quiet while the machine is connected", async () => {
    await banner(fakeApi(connected));
    expect(document.body.textContent).toBe("");
  });

  it("walks POSIX paths whichever system runs the app", () => {
    expect(parentDirectory("/home/dev/")).toBe("/home");
    expect(parentDirectory("/home")).toBe("/");
    expect(joinDirectory("/", "root")).toBe("/root");
    expect(joinDirectory("/home/dev", "app")).toBe("/home/dev/app");
  });
});

describe("connections page", () => {
  const connected: TargetStatus = { ...idle, phase: "connected", step: { step: "ready" }, info: { protocol: 2, version: "1", os: "linux", arch: "x86_64", home: "/home/dev", shell: null, machineIdentity: "m" } };
  const fakeTerminals = (shells: Partial<TerminalInfo>[] = []) => {
    const close = vi.fn(async () => {});
    const client = { list: vi.fn(async () => shells), close } as unknown as TerminalApi;
    return { close, terminals: () => client };
  };
  const page = async (api: TargetApi, terminals = fakeTerminals().terminals, props: { onProjectReady?: () => void; visible?: boolean } = {}) => {
    await act(async () => { root.render(<I18nProvider language="en"><ExecutionTargets api={api} terminals={terminals} {...props} /></I18nProvider>); });
    await act(async () => { await Promise.resolve(); });
  };
  const withHost = (status: TargetStatus) => fakeApi(status, { list: vi.fn(async () => [{ ...entry, status }]) });
  const card = () => document.querySelector<HTMLElement>(".target-card")!;
  const doubleClick = async (element: Element) => { await act(async () => { element.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })); }); await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); }); };
  const tabs = () => [...document.querySelectorAll('[role="tab"]')].map(item => item.textContent);

  it("describes an empty list", async () => {
    await page(fakeApi(idle));
    expect(document.body.textContent).toContain("No remote hosts yet.");
  });

  it("keeps the host list as a first tab that can't be closed", async () => {
    await page(withHost(connected));
    expect(tabs()).toEqual(["Remote hosts"]);
    expect(document.querySelector(".tab-bar-close")).toBeNull();
  });

  it("shows each host as a card under its alias", async () => {
    await page(withHost(idle));
    expect(card().textContent).toContain("Build");
    expect(card().textContent).toContain("build");
    // A card is a host's settings: connecting belongs to what is opened from it.
    expect(card().querySelector('[role="status"]')).toBeNull();
    await act(async () => {});
    await page(withHost({ ...idle, phase: "failed", error: { kind: "auth_failed", detail: "denied" } }));
    expect(card().textContent).not.toContain("Sign-in failed");
  });

  it("asks for the alias first when adding a host", async () => {
    await renderDialog(fakeApi(idle), false, null);
    const first = document.querySelector(".target-form > label")!;
    expect(first.textContent).toBe("Alias");
    expect(first.closest("details")).toBeNull();
  });

  it("does nothing on a single click, and connects in a new tab on a double click", async () => {
    const api = withHost({ ...idle, phase: "connecting", step: { step: "connecting" } });
    vi.mocked(api.list).mockResolvedValue([entry]);
    await page(api);
    await act(async () => { card().querySelector<HTMLButtonElement>(".target-card-open")!.dispatchEvent(new MouseEvent("click", { bubbles: true, detail: 1 })); });
    expect(tabs()).toEqual(["Remote hosts"]);
    await doubleClick(card());
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
    expect(tabs()).toEqual(["Remote hosts", "Build"]);
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("Build");
    expect(document.querySelector(".target-steps")).not.toBeNull();
    expect(document.querySelector(".target-form")).toBeNull();
  });

  it("opens from the keyboard too", async () => {
    await page(withHost(connected));
    await act(async () => { card().querySelector<HTMLButtonElement>(".target-card-open")!.click(); });
    expect(tabs()).toEqual(["Remote hosts", "Build"]);
  });

  const action = (label: string) => card().querySelector<HTMLButtonElement>(`.target-row-actions [aria-label="${label}"]`)!;

  it("does not take a double click on an action for opening the host", async () => {
    await page(withHost(connected));
    await doubleClick(action("Edit"));
    expect(tabs()).toEqual(["Remote hosts"]);
  });

  it("offers its actions as named icons, ending with connect", async () => {
    await page(withHost(connected), undefined, { onProjectReady: () => {} });
    const actions = [...card().querySelectorAll(".target-row-actions button")];
    expect(actions.map(item => item.getAttribute("aria-label"))).toEqual(["Open a project", "Edit", "Remove", "Connect", "Check for agents again"]);
    expect(actions.every(item => item.querySelector("svg") && !item.textContent)).toBe(true);
    expect(button("Disconnect")).toBeUndefined();
  });

  describe("agents on a card", () => {
    const flush = async () => { await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); }); };
    const refresh = () => card().querySelector<HTMLButtonElement>('[aria-label="Check for agents again"]')!;

    it("shows what was found even with nothing connected, the ones not ready dimmed", async () => {
      const api = fakeApi(idle, { list: vi.fn(async () => [{ ...entry, agents: [
        { providerId: "kimi", version: "2", status: "ready" as const },
        { providerId: "codex", version: "1", status: "not_configured" as const },
      ] }]) });
      await page(api);
      const icons = [...card().querySelectorAll(".target-card-agents li")];
      expect(icons.map(icon => icon.getAttribute("data-tooltip")).sort()).toEqual(["Codex · Sign in on the server", "Kimi · Ready"]);
      expect(icons.find(icon => icon.getAttribute("aria-label") === "Codex · Sign in on the server")!.className).toBe("target-card-agent-unready");
      expect(api.connect).not.toHaveBeenCalled();
    });

    it("says so when none were found", async () => {
      await page(withHost(idle));
      expect(card().textContent).toContain("No agents found yet.");
    });

    it("looks again on an open connection without touching it", async () => {
      const api = withHost(connected);
      await page(api);
      await act(async () => { refresh().click(); });
      expect(api.rescan).toHaveBeenCalledWith("tgt_one");
      expect(api.connect).not.toHaveBeenCalled();
      expect(api.release).not.toHaveBeenCalled();
    });

    it("connects only for as long as looking takes when nothing else uses the host", async () => {
      let status: TargetStatus = idle;
      const api = fakeApi(idle, { status: vi.fn(async () => status), list: vi.fn(async () => [{ ...entry, status }]), connect: vi.fn(async () => { status = { ...connected, scanning: true }; }) });
      await page(api);
      await act(async () => { refresh().click(); });
      await flush();
      expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
      expect(api.release).not.toHaveBeenCalled();
      expect(refresh()).toBeNull();
      expect(card().textContent).toContain("Looking for agents");
      status = connected;
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 450)); });
      expect(api.release).toHaveBeenCalledWith("tgt_one");
    });

    it("hides the refresh while a scan runs, keeping the icons", async () => {
      const api = fakeApi(idle, { list: vi.fn(async () => [{ ...entry, status: { ...connected, scanning: true }, agents: [{ providerId: "kimi", version: "2", status: "ready" as const }] }]) });
      await page(api);
      expect(refresh()).toBeNull();
      expect(card().querySelectorAll(".target-card-agents li").length).toBe(1);
    });

    it("doesn't open a shell from a double click on refresh", async () => {
      const api = withHost(idle);
      await page(api);
      await doubleClick(refresh());
      expect(tabs()).toEqual(["Remote hosts"]);
    });
  });

  it("opens a shell tab from the connect arrow, the same as a double click", async () => {
    const api = withHost(idle);
    await page(api);
    await act(async () => { action("Connect").click(); });
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
    expect(tabs()).toEqual(["Remote hosts", "Build"]);
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("Build");
  });

  it("opens one tab when the connect arrow itself is double-clicked", async () => {
    await page(withHost(connected));
    for (const detail of [1, 2]) await act(async () => { action("Connect").dispatchEvent(new MouseEvent("click", { bubbles: true, detail })); });
    await doubleClick(action("Connect"));
    expect(tabs()).toEqual(["Remote hosts", "Build"]);
  });

  it("connects first when opening a project on a host that isn't connected", async () => {
    const api = withHost(idle);
    await page(api, undefined, { onProjectReady: () => {} });
    await act(async () => { action("Open a project").click(); });
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
    expect(document.querySelector(".target-steps")).not.toBeNull();
  });

  it.each(["Cancel", "Close", "Escape"])("closes the shared folder dialog with %s without reopening or creating a project", async closeWith => {
    const api = withHost(connected);
    const ready = vi.fn();
    await page(api, undefined, { onProjectReady: ready });
    await act(async () => { action("Open a project").click(); });
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!;
    expect(dialog.querySelector(".target-browser")).not.toBeNull();
    expect(dialog.closest(".remote-page")).toBeNull();
    expect(container.contains(dialog)).toBe(false);
    await act(async () => {
      if (closeWith === "Escape") dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      else if (closeWith === "Close") dialog.querySelector<HTMLButtonElement>('[aria-label="Close"]')!.click();
      else button("Cancel")!.click();
    });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)); });
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(api.release).toHaveBeenCalledWith("tgt_one");
    expect(api.createProject).not.toHaveBeenCalled();
    expect(ready).not.toHaveBeenCalled();
  });

  it("opens a shell at once on a connected host without connecting again, one tab per shell", async () => {
    const api = withHost(connected);
    await page(api);
    await doubleClick(card());
    await doubleClick(card());
    expect(api.connect).not.toHaveBeenCalled();
    expect(tabs()).toEqual(["Remote hosts", "Build", "Build · 2"]);
    expect(document.querySelectorAll('[data-testid="shell"]').length).toBe(2);
    expect(document.querySelectorAll('[data-testid="shell"][data-visible="true"]').length).toBe(1);
  });

  it("moves from the connection to a shell once connected", async () => {
    const connecting = { ...idle, phase: "connecting" as const, step: { step: "connecting" as const } };
    const api = fakeApi(connecting, { list: vi.fn(async () => [{ ...entry, status: connecting }]) });
    await page(api);
    await doubleClick(card());
    expect(document.querySelector('[data-testid="shell"]')).toBeNull();
    vi.mocked(api.list).mockResolvedValue([{ ...entry, status: connected }]);
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 450)); });
    expect(document.querySelector('[data-testid="shell"]')).not.toBeNull();
  });

  it("stops the shell when its tab closes, and returns to a neighbour", async () => {
    const { close, terminals } = fakeTerminals();
    const api = withHost(connected);
    await page(api, terminals);
    await doubleClick(card());
    await act(async () => { button("shell started")!.click(); });
    await act(async () => { document.querySelector<HTMLButtonElement>(".tab-bar-close")!.click(); });
    expect(close).toHaveBeenCalledWith(expect.any(String), "e1");
    expect(tabs()).toEqual(["Remote hosts"]);
    expect(api.release).toHaveBeenCalledWith("tgt_one");
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("Remote hosts");
  });

  it("keeps the connection until the last tab of that host closes", async () => {
    const api = withHost(connected);
    await page(api);
    await doubleClick(card());
    await doubleClick(card());
    await act(async () => { document.querySelector<HTMLButtonElement>(".tab-bar-close")!.click(); });
    expect(tabs()).toEqual(["Remote hosts", "Build · 2"]);
    expect(api.release).not.toHaveBeenCalled();
    await act(async () => { document.querySelector<HTMLButtonElement>(".tab-bar-close")!.click(); });
    expect(api.release).toHaveBeenCalledTimes(1);
  });

  it("offers to reconnect a lost shell in the same tab, with a new shell", async () => {
    await page(withHost(connected));
    await doubleClick(card());
    const first = document.querySelector('[data-testid="shell"]')!.getAttribute("data-operation");
    expect(document.querySelector(".target-terminal-ended")).toBeNull();
    await act(async () => { button("shell lost")!.click(); });
    await act(async () => { button("Reconnect")!.click(); });
    expect(tabs()).toEqual(["Remote hosts", "Build"]);
    expect(document.querySelector('[data-testid="shell"]')!.getAttribute("data-operation")).not.toBe(first);
    expect(document.querySelector(".target-terminal-ended")).toBeNull();
  });

  it("carries on with a new shell in the tab of one lost to a restart", async () => {
    const { terminals, close } = fakeTerminals([{ id: "lost", epoch: "e", status: "unknown" }]);
    const api = withHost(idle);
    await page(api, terminals);
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    expect(tabs()).toEqual(["Remote hosts", "Build"]);
    expect(close).toHaveBeenCalledWith("lost", "e");
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
    expect(document.querySelector(".target-terminal-ended")).toBeNull();
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("Remote hosts");
    expect(document.querySelector('[data-testid="shell"]')).toBeNull();
  });

  it("starts that new shell under its own identity once connected", async () => {
    const { terminals } = fakeTerminals([{ id: "lost", epoch: "e", status: "unknown" }]);
    await page(withHost(connected), terminals);
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    expect(document.querySelector('[data-testid="shell"]')!.getAttribute("data-operation")).not.toBe("lost");
  });

  it("doesn't leave a reconnected shell's old record behind", async () => {
    const { terminals, close } = fakeTerminals();
    await page(withHost(connected), terminals);
    await doubleClick(card());
    await act(async () => { button("shell lost")!.click(); });
    await act(async () => { button("Reconnect")!.click(); });
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("puts back the tabs of shells still running, leaving the list selected", async () => {
    const { terminals } = fakeTerminals([{ id: "s1", epoch: "e", status: "running" }, { id: "s2", epoch: "e", status: "exited" }]);
    await page(withHost(connected), terminals);
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    expect(tabs()).toEqual(["Remote hosts", "Build"]);
    expect(document.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("Remote hosts");
  });

  it("keeps tabs while hidden, without polling", async () => {
    const api = withHost(connected);
    await page(api);
    await doubleClick(card());
    const calls = vi.mocked(api.list).mock.calls.length;
    await page(api, undefined, { visible: false });
    expect(tabs()).toEqual(["Remote hosts", "Build"]);
    expect(document.querySelector('[data-testid="shell"]')!.getAttribute("data-visible")).toBe("false");
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 450)); });
    expect(vi.mocked(api.list).mock.calls.length).toBe(calls);
  });

  it("removes a connection only after confirmation", async () => {
    const api = fakeApi(idle, { list: vi.fn(async () => [entry]) });
    const inDialog = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("dialog button")].find(item => item.textContent === text)!;
    await page(api);
    await act(async () => { action("Remove").click(); });
    expect(document.querySelector("dialog")?.textContent).toContain("Build");
    await act(async () => { inDialog("Cancel").click(); });
    expect(api.remove).not.toHaveBeenCalled();
    await act(async () => { action("Remove").click(); });
    await act(async () => { inDialog("Remove").click(); });
    expect(api.remove).toHaveBeenCalledWith("tgt_one");
    expect(document.querySelector("dialog")).toBeNull();
  });
});
