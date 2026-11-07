// @vitest-environment jsdom
import { act } from "react";
import { webcrypto } from "node:crypto";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { CoreClient, TaskId } from "@dogcodes/protocol";
import { ContextMenuHost } from "./ContextMenu";
import { TurnFiles } from "./TurnFiles";
import { RemoteConversationContext, messageFileMenu } from "./workspaceFileMenu";
import { copyPendingText } from "./clipboard";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: () => false }));
let root: Root, host: HTMLDivElement;
const writeText = vi.fn(async () => {});
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("crypto", webcrypto);
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
  writeText.mockClear();
  vi.mocked(invoke).mockReset().mockImplementation(async command => command === "core_workspace_file_path" ? "/repo/src/a.ts" : { applications: [], error: null, absolutePath: "/repo/src/a.ts" });
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });

const labels = (entries: ReturnType<typeof messageFileMenu>) => entries.filter(entry => entry && entry !== "separator").map(entry => (entry as { label: string }).label);
const menuLabels = () => [...document.querySelectorAll('.context-menu [role="menuitem"]')].map(item => item.textContent);

async function artifact(files: { path: string; status: string }[]) {
  const bytes = new TextEncoder().encode(JSON.stringify({ files, patch: "" }));
  const hash = Array.from(new Uint8Array(await webcrypto.subtle.digest("SHA-256", bytes)), n => n.toString(16).padStart(2, "0")).join("");
  const metadata = { artifactId: `art_${hash.slice(0, 8)}` as `art_${string}`, payloadSha256: hash, byteLength: bytes.length, mediaType: "application/json", createdAt: "2026-09-11T00:00:00Z", redactionRulesetVersion: 0, redactionCount: 0 };
  const client = { readArtifact: vi.fn(async () => ({ metadata, offset: 0, bytes: Array.from(bytes), nextOffset: null })) } as unknown as CoreClient;
  return { metadata, client };
}
const rightClick = (target: Element) => act(async () => { target.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })); });

it("offers only what still makes sense for where a message's file is", () => {
  const taskId = "task", path = "src/a.ts";
  expect(labels(messageFileMenu({ taskId, path, remote: false }))).toEqual(["打开", "用其他应用打开…", expect.any(String), "复制路径", "复制相对路径"]);
  expect(labels(messageFileMenu({ taskId, path, remote: true }))).toEqual(["复制路径", "复制相对路径"]);
  expect(labels(messageFileMenu({ taskId, path, remote: false, deleted: true }))).toEqual(["复制相对路径"]);
  expect(labels(messageFileMenu({ taskId, path: "/repo/src/a.ts", remote: true }))).toEqual(["复制路径"]);
  expect(labels(messageFileMenu({ taskId, path: "C:\\repo\\a.ts", remote: true, deleted: true }))).toEqual(["复制路径"]);
});

it("writes a path it must first resolve within the click", async () => {
  const write = vi.fn(async (items: { types: Record<string, Promise<Blob>> }[]) => { await items[0].types["text/plain"]; });
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText, write } });
  vi.stubGlobal("ClipboardItem", class { constructor(public types: Record<string, Promise<Blob>>) {} });
  let finish!: (path: string) => void;
  const copied = copyPendingText(new Promise(resolve => { finish = resolve; }));
  // Called before the text arrives: waiting for it would lose the click in WebKit.
  expect(write).toHaveBeenCalledTimes(1);
  finish("/repo/src/a.ts");
  await copied;
  const blob = await write.mock.calls[0][0][0].types["text/plain"];
  expect(blob.type).toBe("text/plain");
  expect(blob.size).toBe("/repo/src/a.ts".length);
});

it("gives a remote conversation's file cards no local actions, and resolves their path on request", async () => {
  const { metadata, client } = await artifact([{ path: "src/a.ts", status: "M" }]);
  const show = (remote: boolean) => act(async () => root.render(<RemoteConversationContext.Provider value={remote}><ContextMenuHost /><TurnFiles taskId={"tsk_a" as TaskId} metadata={metadata} client={client} /></RemoteConversationContext.Provider>));
  await show(true);
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 20)); });
  const row = host.querySelector(".turn-summary-file")!;
  expect(host.querySelector(".open-file-control")).toBeNull();
  expect(invoke).not.toHaveBeenCalledWith("core_workspace_file_applications", expect.anything());
  await rightClick(row);
  expect(menuLabels().slice(1)).toEqual(["复制路径", "复制相对路径"]);
  await act(async () => [...document.querySelectorAll<HTMLButtonElement>('.context-menu [role="menuitem"]')].find(item => item.textContent === "复制路径")!.click());
  expect(invoke).toHaveBeenCalledWith("core_workspace_file_path", { taskId: "tsk_a", path: "src/a.ts" });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
  expect(writeText).toHaveBeenCalledWith("/repo/src/a.ts");

  await show(false);
  expect(host.querySelector(".open-file-control")).not.toBeNull();
  await rightClick(host.querySelector(".turn-summary-file")!);
  expect(menuLabels()).toContain("打开");
});

it("puts the file menu on an edit card's heading and leaves its code to the text menu", async () => {
  const { metadata, client } = await artifact([{ path: "src/b.ts", status: "D" }]);
  await act(async () => root.render(<><ContextMenuHost /><TurnFiles taskId={"tsk_a" as TaskId} metadata={metadata} client={client} operation /></>));
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 20)); });
  await rightClick(host.querySelector(".inline-edit-heading")!);
  expect(menuLabels()).toEqual(["复制相对路径"]);
  await act(async () => { document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
  await act(async () => {
    const details = host.querySelector<HTMLDetailsElement>(".inline-file-edit")!;
    details.open = true; details.dispatchEvent(new Event("toggle"));
  });
  await rightClick(host.querySelector(".inline-edit-code")!);
  expect(menuLabels()).not.toContain("复制相对路径");
});
