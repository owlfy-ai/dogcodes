// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ExecutionTargets, TargetConnectionDialog, TargetOfflineBanner, TargetOfflinePanel } from "./ExecutionTargets";
import { I18nProvider } from "./i18n";
import { connectionFromForm, joinDirectory, parentDirectory, stepStates, targetRoute, type TargetApi, type TargetStatus } from "./executionTargetClient";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn(() => false) }));

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
    expect(container.querySelector('[role="radio"][aria-checked="true"]')?.textContent).toBe("System default");
    expect(container.querySelector('input[type="password"]')).toBeNull();
  });

  it("saves the route without the secret and passes the secret only to connect", async () => {
    const api = fakeApi(idle);
    await renderDialog(api);
    await setValue(container.querySelector('input[type="password"]')!, "s3cret");
    await act(async () => { container.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    expect(JSON.stringify(vi.mocked(api.save).mock.calls[0])).not.toContain("s3cret");
    expect(api.connect).toHaveBeenCalledWith("tgt_new", "s3cret");
  });

  it("imports an SSH config host as an alias, leaving its values to ssh", async () => {
    const api = fakeApi(idle, { configHosts: vi.fn(async () => ["gpu"]), resolveHost: vi.fn(async () => ({ host: "10.0.0.2", user: "dev", port: 22 })) });
    await renderDialog(api, false, null);
    const select = container.querySelector("select")!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!.set!.call(select, "gpu");
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(container.querySelector<HTMLInputElement>("input[required]")!.value).toBe("gpu");
    expect(container.querySelector(".target-route")?.textContent).toBe("dev@10.0.0.2:22");
    await act(async () => { container.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })); });
    expect(vi.mocked(api.save).mock.calls[0]![2]).toEqual({ destination: "gpu", auth: "system" });
  });
});

describe("connection progress", () => {
  it("never saves a host key answer as a secret", async () => {
    const api = fakeApi({ ...idle, phase: "connecting", step: { step: "connecting" }, prompt: { id: "p1", kind: "host_key", text: "fingerprint SHA256:abc" } });
    await renderDialog(api, true);
    expect(container.textContent).toContain("SHA256:abc");
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
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("identity has changed");
    expect(button("Trust and continue")).toBeUndefined();
  });

  it("warns when a saved route reaches a different machine", async () => {
    await renderDialog(fakeApi({ ...idle, phase: "connected", identityChanged: true, info: { protocol: 1, version: "1", os: "linux", arch: "x86_64", home: "/h", shell: null, machineIdentity: "m" } }), true);
    expect(container.textContent).toContain("different machine");
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
    expect(container.textContent).toContain("Kimi Code");
    expect(container.textContent).toContain("Sign in on the server");
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

  it("reconnects from the banner without asking anything when nobody is needed", async () => {
    const api = fakeApi(idle);
    await banner(api);
    expect(container.textContent).toContain("not connected");
    vi.mocked(api.status).mockResolvedValue(connected);
    await act(async () => { button("Reconnect")!.click(); });
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
    await flush();
    expect(container.textContent).toBe("");
    expect(container.querySelector("[role=dialog]")).toBeNull();
  });

  it("shows the connection dialog in place when a password is needed", async () => {
    const api = fakeApi(idle, { list: vi.fn(async () => [entry]) });
    await banner(api);
    vi.mocked(api.status).mockResolvedValue({ ...idle, phase: "connecting", prompt: { id: "p", kind: "password", text: "Password for build" } });
    await act(async () => { button("Reconnect")!.click(); });
    await flush();
    expect(container.textContent).toContain("Password for build");
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
    expect(container.textContent).not.toContain("panel content");
    expect(container.querySelector("aside .review-empty-action")?.textContent).toBe("Reconnect");
  });

  it.each([".target-offline button", "aside .review-empty-action"])("connecting from %s updates both views at once", async selector => {
    const api = fakeApi(idle);
    await both(api);
    vi.mocked(api.status).mockResolvedValue(connected);
    await act(async () => { container.querySelector<HTMLButtonElement>(selector)!.click(); });
    expect(api.connect).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".target-offline")).toBeNull();
    expect(container.textContent).toBe("panel content");
  });

  it("asks for a password once, in the view that was clicked", async () => {
    const api = fakeApi(idle, { list: vi.fn(async () => [entry]) });
    await both(api);
    vi.mocked(api.status).mockResolvedValue({ ...idle, phase: "connecting", prompt: { id: "p", kind: "password", text: "Password for build" } });
    await act(async () => { container.querySelector<HTMLButtonElement>("aside .review-empty-action")!.click(); });
    await flush();
    await flush();
    expect(container.querySelectorAll("[role=dialog]").length).toBe(1);
  });

  it("stays quiet while the machine is connected", async () => {
    await banner(fakeApi(connected));
    expect(container.textContent).toBe("");
  });

  it("walks POSIX paths whichever system runs the app", () => {
    expect(parentDirectory("/home/dev/")).toBe("/home");
    expect(parentDirectory("/home")).toBe("/");
    expect(joinDirectory("/", "root")).toBe("/root");
    expect(joinDirectory("/home/dev", "app")).toBe("/home/dev/app");
  });
});

describe("connections page", () => {
  it("opens the new connection dialog when asked from the project picker", async () => {
    await act(async () => { root.render(<I18nProvider language="en"><ExecutionTargets api={fakeApi(idle)} requestNew={1} /></I18nProvider>); });
    expect(container.querySelector('[role="dialog"]')).not.toBeNull();
  });

  it("describes an empty list", async () => {
    await act(async () => { root.render(<I18nProvider language="en"><ExecutionTargets api={fakeApi(idle)} /></I18nProvider>); });
    expect(container.textContent).toContain("No remote connections yet.");
  });

  it("connects from the list without showing the settings form", async () => {
    const api = fakeApi({ ...idle, phase: "connecting", step: { step: "connecting" } }, { list: vi.fn(async () => [entry]) });
    await act(async () => { root.render(<I18nProvider language="en"><ExecutionTargets api={api} /></I18nProvider>); });
    await act(async () => { button("Connect")!.click(); });
    expect(api.connect).toHaveBeenCalledWith("tgt_one", null);
    expect(container.querySelector(".target-steps")).not.toBeNull();
    expect(container.querySelector(".target-form")).toBeNull();
  });

  it("removes a connection only after confirmation", async () => {
    const api = fakeApi(idle, { list: vi.fn(async () => [entry]) });
    const inDialog = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("dialog button")].find(item => item.textContent === text)!;
    await act(async () => { root.render(<I18nProvider language="en"><ExecutionTargets api={api} /></I18nProvider>); });
    await act(async () => { button("Remove")!.click(); });
    expect(document.querySelector("dialog")?.textContent).toContain("Build");
    await act(async () => { inDialog("Cancel").click(); });
    expect(api.remove).not.toHaveBeenCalled();
    await act(async () => { button("Remove")!.click(); });
    await act(async () => { inDialog("Remove").click(); });
    expect(api.remove).toHaveBeenCalledWith("tgt_one");
    expect(document.querySelector("dialog")).toBeNull();
  });
});
