// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { App } from "./App";
import { DemoCoreClient } from "./demoClient";

let root: Root, host: HTMLDivElement;

beforeEach(async () => {
  localStorage.clear();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", clearTimeout);
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  HTMLElement.prototype.scrollIntoView = vi.fn();
  const client = new DemoCoreClient();
  const snapshot = await client.bootstrap();
  snapshot.projects = snapshot.projects.map(project => ({ ...project, targetId: "tgt_server" }));
  vi.spyOn(client, "bootstrap").mockResolvedValue(snapshot);
  vi.spyOn(client, "timeline").mockResolvedValue([]);
  vi.spyOn(client, "subscribe").mockResolvedValue(() => {});
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root.render(<App client={client} />));
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  localStorage.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("offers to reconnect in the right panel while the server is not connected", async () => {
  const toggle = host.querySelector<HTMLButtonElement>('[aria-label="显示右侧边栏"]')!;
  expect(toggle.disabled).toBe(false);
  await act(async () => toggle.click());
  expect(host.querySelector(".review-panel .review-empty-action")?.textContent).toBe("重新连接");
});
