// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { I18nProvider } from "./i18n";
import { AppUpdateNotice } from "./AppUpdateNotice";
import { installFailure, laterVersion, rememberLater, shouldOfferUpdate, type AppUpdate, type AppUpdateApi } from "./appUpdate";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

let root: Root, host: HTMLDivElement;
const update = (version: string): AppUpdate => ({ version, currentVersion: "0.1.3" });

/** A backend whose download may already be ready, and which can announce more later. */
function fakeApi(ready: AppUpdate | null = null, install: () => Promise<void> = async () => {}) {
  let announce: (next: AppUpdate) => void = () => {};
  const unlisten = vi.fn();
  const api: AppUpdateApi = {
    ready: vi.fn(async () => ready),
    onReady: vi.fn(async callback => { announce = callback; return unlisten; }),
    install: vi.fn(install),
  };
  return { api, unlisten, announce: (next: AppUpdate) => act(async () => announce(next)) };
}
const render = async (api: AppUpdateApi, running = false) => {
  await act(async () => root.render(<I18nProvider language="en"><AppUpdateNotice api={api} running={running} /></I18nProvider>));
};
const notice = () => document.querySelector<HTMLElement>(".app-update-notice");
const button = (text: string) => [...document.querySelectorAll("button")].find(item => item.textContent === text)!;
const click = (text: string) => act(async () => button(text).click());

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); });

it("offers only a stable download that was not put off and while nothing runs", () => {
  expect(shouldOfferUpdate(null, null, false)).toBe(false);
  expect(shouldOfferUpdate(update("0.1.4"), null, false)).toBe(true);
  expect(shouldOfferUpdate(update("0.1.4"), null, true)).toBe(false);
  expect(shouldOfferUpdate(update("0.1.4"), "0.1.4", false)).toBe(false);
  expect(shouldOfferUpdate(update("0.1.5"), "0.1.4", false)).toBe(true);
});

it("remembers Later per version and survives unavailable storage", () => {
  expect(laterVersion()).toBeNull();
  rememberLater("0.1.4");
  expect(laterVersion()).toBe("0.1.4");
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
  expect(laterVersion()).toBeNull();
  expect(() => rememberLater("0.1.5")).not.toThrow();
});

it("maps install errors to what the person can do about them", () => {
  expect(installFailure("busy")).toBe("busy");
  expect(installFailure("failed")).toBe("failed");
  expect(installFailure("unavailable")).toBe("failed");
  expect(installFailure(new Error("ipc"))).toBe("failed");
});

it("shows nothing until an update has been downloaded", async () => {
  const { api } = fakeApi();
  await render(api);
  expect(api.ready).toHaveBeenCalled();
  expect(notice()).toBeNull();
});

it("shows a download that finished before the window listened, without a dialog", async () => {
  const { api } = fakeApi(update("0.1.4"));
  await render(api);
  expect(notice()?.textContent).toContain("dog.codes 0.1.4 is ready");
  expect(document.querySelector("dialog")).toBeNull();
  expect(notice()?.querySelector("[title]")).toBeNull();
  expect(button("Install and restart")).toBeTruthy();
  expect(button("Later")).toBeTruthy();
});

it("appears when the backend announces a download", async () => {
  const { api, announce } = fakeApi();
  await render(api);
  await announce(update("0.1.4"));
  expect(notice()?.textContent).toContain("0.1.4");
});

it("waits while an Agent turn is running", async () => {
  const { api } = fakeApi(update("0.1.4"));
  await render(api, true);
  expect(notice()).toBeNull();
  await render(api, false);
  expect(notice()).not.toBeNull();
});

it("stays hidden after Later until a newer version arrives", async () => {
  const { api, announce } = fakeApi(update("0.1.4"));
  await render(api);
  await click("Later");
  expect(notice()).toBeNull();
  expect(laterVersion()).toBe("0.1.4");
  expect(api.install).not.toHaveBeenCalled();

  // Reopening the app does not bring the same version back.
  await act(async () => root.unmount());
  root = createRoot(host);
  await render(api);
  expect(notice()).toBeNull();
  await announce(update("0.1.4"));
  expect(notice()).toBeNull();

  await announce(update("0.1.5"));
  expect(notice()?.textContent).toContain("0.1.5");
});

it("installs on request and explains a refusal while tasks still run", async () => {
  let reject: (reason: unknown) => void = () => {};
  const { api } = fakeApi(update("0.1.4"), () => new Promise((_, fail) => { reject = fail; }));
  await render(api);
  await click("Install and restart");
  expect(api.install).toHaveBeenCalledTimes(1);
  expect(button("Installing…").disabled).toBe(true);
  expect(button("Later").disabled).toBe(true);

  await act(async () => reject("busy"));
  expect(notice()?.querySelector("[role=alert]")?.textContent).toBe("Tasks are still running. Install once they finish.");
  expect(button("Install and restart").disabled).toBe(false);

  await click("Install and restart");
  await act(async () => reject("failed"));
  expect(notice()?.querySelector("[role=alert]")?.textContent).toBe("The update couldn't be installed. Try again later.");
});

it("stops listening when it unmounts", async () => {
  const { api, unlisten } = fakeApi();
  await render(api);
  await act(async () => root.unmount());
  expect(unlisten).toHaveBeenCalled();
  root = createRoot(host);
});
