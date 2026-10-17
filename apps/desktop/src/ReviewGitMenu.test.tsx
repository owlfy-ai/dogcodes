// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ReviewGitMenu, ReviewGitRemoteStatus, type GitRemoteNotice } from "./ReviewGitMenu";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./gitOperations", async importOriginal => ({ ...await importOriginal<typeof import("./gitOperations")>(), gitOperations: { run: (args: Record<string, unknown>) => invoke("core_review_git_action", args) } }));
let host: HTMLDivElement, root: Root;
const onBusy = vi.fn(), onChanged = vi.fn(), onCompleted = vi.fn();
function Screen({ disabled = false, git = true, visible = true }) {
  const [notice, setNotice] = useState<GitRemoteNotice | null>(null);
  return <><ReviewGitMenu taskId="task" root={{ id: "repo", path: "/project/repo", git, scopes: [""] }} disabled={disabled} visible={visible} onBusy={onBusy} onChanged={onChanged} onCompleted={onCompleted} onNotice={setNotice}/><ReviewGitRemoteStatus notice={notice}/></>;
}
const render = (props = {}) => act(async () => root.render(<Screen {...props}/>));
const button = (label: string) => [...host.querySelectorAll<HTMLButtonElement>("button")].find(node => node.getAttribute("aria-label") === label || node.textContent === label)!;
const click = (label: string) => act(async () => { expect(button(label)).toBeTruthy(); button(label).click(); });
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  vi.resetAllMocks(); vi.mocked(invoke).mockResolvedValue(true);
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value() { this.open = true; } });
  Object.defineProperty(HTMLDialogElement.prototype, "close", { configurable: true, value() { this.open = false; } });
  host = document.createElement("div"); document.body.append(host); root = createRoot(host); await render();
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); Reflect.deleteProperty(HTMLDialogElement.prototype, "showModal"); Reflect.deleteProperty(HTMLDialogElement.prototype, "close"); });
it("opens a floating menu, supports arrow keys and restores trigger focus on Escape", async () => {
  await click("更多 Git 操作"); expect(host.querySelector('[role="menu"]')).not.toBeNull();
  expect(document.activeElement).toBe(button("拉取"));
  await act(async () => button("拉取").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })));
  expect(document.activeElement).toBe(button("同步更改…"));
  await act(async () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  expect(host.querySelector('[role="menu"]')).toBeNull(); expect(document.activeElement).toBe(button("更多 Git 操作"));
});
it("pulls the selected repository, reports progress and completion, and refreshes without duplicate requests", async () => {
  let resolve!: (value: boolean) => void; vi.mocked(invoke).mockReturnValueOnce(new Promise(done => { resolve = done; }));
  await click("更多 Git 操作"); await click("拉取");
  expect(host.textContent).toContain("正在拉取"); expect(button("更多 Git 操作").disabled).toBe(true);
  await click("更多 Git 操作"); expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_action", { taskId: "task", rootId: "repo", action: "pull", message: "", stageAll: false });
  await act(async () => resolve(true));
  expect(onCompleted).toHaveBeenCalledExactlyOnceWith("pull"); expect(onChanged).toHaveBeenCalledOnce();
  expect(onBusy.mock.calls).toEqual([[true], [false]]); expect(host.textContent).toContain("拉取完成"); expect(host.querySelector(".loading-status")).toBeNull();
});
it("confirms synchronization, cancels without invoking Git, and sends only sync after confirmation", async () => {
  await click("更多 Git 操作"); await click("同步更改…");
  expect(host.querySelector("dialog")?.open).toBe(true); expect(document.activeElement).toBe(host.querySelector('[data-dialog-title]'));
  expect(invoke).not.toHaveBeenCalled(); await click("取消"); expect(invoke).not.toHaveBeenCalled();
  await click("更多 Git 操作"); await click("同步更改…"); await click("同步更改");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_action", { taskId: "task", rootId: "repo", action: "sync", message: "", stageAll: false });
  expect(host.querySelector("dialog")).toBeNull(); expect(host.textContent).toContain("同步完成"); expect(onCompleted).toHaveBeenCalledExactlyOnceWith("sync");
});
it("refreshes partial pull results and collapses conflict details without reporting sync success", async () => {
  vi.mocked(invoke).mockRejectedValueOnce("CONFLICT (content): Merge conflict in file");
  await click("更多 Git 操作"); await click("同步更改…"); await click("同步更改");
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("存在合并冲突");
  expect(host.querySelector("details")?.open).toBe(false); expect(host.querySelector("pre")?.textContent).toContain("CONFLICT");
  expect(onChanged).toHaveBeenCalledOnce(); expect(onCompleted).not.toHaveBeenCalled(); expect(onBusy).toHaveBeenLastCalledWith(false);
});
it("disables unavailable repositories and closes the menu when its panel is hidden", async () => {
  await render({ git: false }); await click("更多 Git 操作"); expect(button("拉取").disabled).toBe(true); expect(button("同步更改…").disabled).toBe(true);
  await render({ visible: false }); expect(host.querySelector('[role="menu"]')).toBeNull();
  await render({ disabled: true }); expect(button("更多 Git 操作").disabled).toBe(true); expect(invoke).not.toHaveBeenCalled();
});
