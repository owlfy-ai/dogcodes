// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { TurnUndoAction } from "./TurnUndoAction";
import type { ArtifactMetadata } from "@dogcodes/protocol";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const watchers = vi.hoisted(() => ({ refresh: undefined as (() => void) | undefined }));
vi.mock("./reviewWatch", () => ({ subscribeReviewChanges: (_task: string, refresh: () => void) => { watchers.refresh = refresh; return () => {}; } }));
import { reviewMutations } from "./reviewMutations";
const metadata: ArtifactMetadata = {artifactId:"art_one",payloadSha256:"sha",mediaType:"application/vnd.dogcodes.turn-changes.v1+json",byteLength:2,createdAt:"2026-09-15T00:00:00Z",redactionRulesetVersion:0,redactionCount:0};
let root: Root, host: HTMLDivElement;
const button = (text: string) => [...document.querySelectorAll("button")].find(b => b.textContent === text)!;
async function click(text: string) { await act(async () => button(text).click()); }
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  localStorage.clear();
  invoke.mockReset().mockResolvedValue(false);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); vi.unstubAllGlobals(); });
async function render() { await act(async () => root.render(<details><summary>2 个文件已更改<TurnUndoAction taskId="tsk_one" metadata={metadata} count={2} /></summary></details>)); }
it("requires confirmation and cancel never changes files or toggles the summary", async () => {
  await render(); await click("撤销");
  expect(host.querySelector("details")?.open).toBe(false);
  expect(document.querySelector("dialog")?.open).toBe(true);
  expect(document.activeElement).toBe(document.querySelector("[data-dialog-title]"));
  expect(invoke.mock.calls.some(([name]) => name === "core_turn_undo")).toBe(false);
  await click("取消");
  expect(document.querySelector("dialog")).toBeNull();
  expect(invoke.mock.calls.some(([name]) => name === "core_turn_undo")).toBe(false);
});
it("sends only the verified artifact identity after confirmation and disables duplicate undo", async () => {
  await render(); await click("撤销"); await click("确认撤销");
  expect(invoke).toHaveBeenCalledWith("core_turn_undo", {operationId:expect.any(String),request:{taskId:"tsk_one",artifactId:"art_one",expectedPayloadSha256:"sha",confirmed:true}});
  expect(button("已撤销").disabled).toBe(true);
  expect(document.querySelector("dialog")).toBeNull();
});
it("keeps conflicts visible in the dialog without marking the turn undone", async () => {
  invoke.mockImplementation(async (name: string) => { if (name === "core_turn_undo") throw "reply lost"; if (name === "core_review_file_operation") return { request: reviewMutations.entries("tsk_one")[0].request, outcome: { status: "failed", message: "file.txt 在本轮结束后又有修改，未撤销任何文件" } }; return false; });
  await render(); await click("撤销"); await click("确认撤销");
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("后又有修改");
  expect(button("确认撤销")).toBeUndefined();
  expect(button("核实撤销").disabled).toBe(false);
});
it("restores persisted undone state when revisiting the turn", async () => {
  invoke.mockResolvedValue(true); await render(); expect(button("已撤销").disabled).toBe(true);
});
it("resolves a lost undo reply through its receipt without sending undo twice", async () => {
  invoke.mockImplementation(async (name: string) => {
    if (name === "core_turn_undo") throw "disconnected";
    if (name === "core_review_file_operation") return { request: reviewMutations.entries("tsk_one")[0].request, outcome: { status: "succeeded", result: null } };
    return false;
  });
  await render(); await click("撤销"); await click("确认撤销");
  expect(button("已撤销").disabled).toBe(true);
  expect(invoke.mock.calls.filter(([name]) => name === "core_turn_undo")).toHaveLength(1);
});
it("keeps unknown undo identity after reopening and hides a second confirmation", async () => {
  invoke.mockImplementation(async (name: string) => { if (name === "core_turn_undo_status") return false; throw "offline"; });
  await render(); await click("撤销"); await click("确认撤销");
  expect(button("确认撤销")).toBeUndefined();
  const saved = reviewMutations.entries("tsk_one")[0];
  await act(async () => root.render(null)); await render();
  await click("核实撤销");
  expect(button("确认撤销")).toBeUndefined();
  expect(reviewMutations.entries("tsk_one")[0].request).toEqual(saved.request);
  expect(invoke.mock.calls.filter(([name]) => name === "core_turn_undo")).toHaveLength(1);
});
it("refreshes undone state from another client's invalidation", async () => {
  await render();
  invoke.mockResolvedValue(true);
  await act(async () => watchers.refresh?.());
  expect(button("已撤销").disabled).toBe(true);
  expect(invoke.mock.calls.some(([name]) => name === "core_turn_undo")).toBe(false);
});
