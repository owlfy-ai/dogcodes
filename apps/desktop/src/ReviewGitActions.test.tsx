// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ReviewGitActions } from "./ReviewGitActions";
import { setAgentEnabled } from "./agentPreferences";
import { COMMIT_AGENT_KEY, COMMIT_LANGUAGE_KEY } from "./CommitSummarySettings";
import { SKIP_STAGE_ALL_CONFIRMATION_KEY, setSkipStageAllConfirmation } from "./gitCommitPreferences";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./gitOperations", async importOriginal => ({ ...await importOriginal<typeof import("./gitOperations")>(), gitOperations: { run: (args: Record<string, unknown>) => invoke("core_review_git_action", args), advance: (taskId: string, operationId: string) => invoke("core_review_git_advance", { taskId, operationId }) } }));
let container: HTMLDivElement, reactRoot: Root;
const executor = { providerId: "codex", installationId: "ins_test" };
const changed = vi.fn(), onBusy = vi.fn();
const root = { id: "repo", path: "/repo", git: true, scopes: [""] };
const render = (disabled = false, git = true, key = "repo", hasChanges = true, snapshotId = "snapshot", pulledRevision = 0, syncedRevision = 0) => act(async () => {
  reactRoot.render(<ReviewGitActions key={key} taskId="task" snapshotId={snapshotId} pulledRevision={pulledRevision} syncedRevision={syncedRevision} root={{ ...root, id: key, git }} hasChanges={hasChanges} disabled={disabled} onChanged={changed} onBusy={onBusy} />);
});
const click = (label: string) => act(async () => {
  const button = [...container.querySelectorAll("button")].find(button => button.getAttribute("aria-label") === label || button.textContent === label);
  expect(button, label).toBeTruthy(); button!.click();
});
const type = (value: string) => act(async () => {
  const input = container.querySelector("input")!;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
});
beforeEach(async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); vi.mocked(invoke).mockReset(); changed.mockReset(); onBusy.mockReset(); localStorage.clear();
  vi.mocked(invoke).mockImplementation(async command => command === "core_bootstrap" ? { installations: [{ ...executor, status: "ready" }] } : command === "core_model_ready_executors" ? [executor] : true);
  container = document.createElement("div"); document.body.append(container); reactRoot = createRoot(container);
  await render();
});
afterEach(async () => { await act(async () => reactRoot.unmount()); container.remove(); localStorage.clear(); vi.unstubAllGlobals(); });
it("renders an inline composer without the old dialogs or standalone icons", () => {
  expect(container.querySelector("input")).toBeTruthy();
  expect(container.querySelector(".review-commit-repository")).toBeNull();
  expect(container.querySelector("dialog")).toBeNull();
  expect(container.querySelector('[aria-label="Git Commit"]')).toBeNull();
  expect(container.querySelector('[aria-label="Git Push"]')).toBeNull();
  expect(container.querySelector<HTMLButtonElement>(".review-commit-buttons > button")!.disabled).toBe(true);
});
it("picks an Agent on first generate, then generates directly and preserves the draft on failure", async () => {
  vi.mocked(invoke).mockImplementation(async command => command === "core_bootstrap" ? { installations: [{ ...executor, status: "ready" }] } : command === "core_model_ready_executors" ? [executor] : "修复图片预览");
  await click("生成提交说明");
  await click("Codex");
  expect(invoke).toHaveBeenLastCalledWith("core_review_commit_summary", { taskId: "task", rootId: "repo", executor, language: "zh-CN" });
  expect(container.querySelector("input")!.value).toBe("修复图片预览");
  expect(JSON.parse(localStorage.getItem(COMMIT_AGENT_KEY)!)).toEqual(executor);
  vi.mocked(invoke).mockReset().mockRejectedValueOnce("Agent 离线");
  await click("生成提交说明");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_commit_summary", { taskId: "task", rootId: "repo", executor, language: "zh-CN" });
  expect(container.querySelector("input")!.value).toBe("修复图片预览");
  expect(container.textContent).toContain("Agent 离线"); expect(changed).not.toHaveBeenCalled();
});
it("choosing an Agent only changes the selection without generating", async () => {
  await click("选择提交说明 Agent"); await click("Codex");
  expect(vi.mocked(invoke).mock.calls.map(([name]) => name)).toEqual(["core_bootstrap", "core_model_ready_executors"]);
  expect(JSON.parse(localStorage.getItem(COMMIT_AGENT_KEY)!)).toEqual(executor);
  expect(container.querySelector(".review-commit-agent")!.textContent).toContain("Codex");
});
it("remembers the chosen summary language and generates with it", async () => {
  await click("选择提交说明语言");
  expect(container.querySelector(".review-language-menu")).toBeTruthy();
  await click("English");
  expect(container.querySelector(".review-language-menu")).toBeNull();
  expect(localStorage.getItem(COMMIT_LANGUAGE_KEY)).toBe("en");
  vi.mocked(invoke).mockImplementation(async command => command === "core_bootstrap" ? { installations: [{ ...executor, status: "ready" }] } : command === "core_model_ready_executors" ? [executor] : "Fix image preview");
  await click("生成提交说明"); await click("Codex");
  expect(invoke).toHaveBeenLastCalledWith("core_review_commit_summary", { taskId: "task", rootId: "repo", executor, language: "en" });
  expect(container.querySelector("input")!.value).toBe("Fix image preview");
});
it("locks language selection while loading a saved Agent and unlocks after generation", async () => {
  localStorage.setItem(COMMIT_AGENT_KEY, JSON.stringify(executor));
  await render(false, true, "saved-agent");
  await click("选择提交说明语言");
  let release!: (value: unknown) => void;
  vi.mocked(invoke).mockImplementation(command => command === "core_bootstrap" ? new Promise(resolve => { release = resolve; }) : command === "core_model_ready_executors" ? Promise.resolve([executor]) : Promise.resolve("summary"));
  await click("生成提交说明");
  expect(container.querySelector(".review-language-menu")).toBeNull();
  expect(container.querySelector<HTMLButtonElement>('[aria-label="选择提交说明语言"]')!.disabled).toBe(true);
  await click("选择提交说明语言");
  expect(container.querySelector(".review-language-menu")).toBeNull();
  await act(async () => release({ installations: [{ ...executor, status: "ready" }] }));
  expect(invoke).toHaveBeenLastCalledWith("core_review_commit_summary", { taskId: "task", rootId: "saved-agent", executor, language: "zh-CN" });
  expect(container.querySelector<HTMLButtonElement>('[aria-label="选择提交说明语言"]')!.disabled).toBe(false);
  await click("选择提交说明语言"); await click("English");
  expect(localStorage.getItem(COMMIT_LANGUAGE_KEY)).toBe("en");
});
it("unlocks language selection after saved Agent loading fails", async () => {
  localStorage.setItem(COMMIT_AGENT_KEY, JSON.stringify(executor));
  await render(false, true, "saved-agent");
  vi.mocked(invoke).mockRejectedValueOnce("Agent 离线");
  await click("生成提交说明");
  expect(container.querySelector<HTMLButtonElement>('[aria-label="选择提交说明语言"]')!.disabled).toBe(false);
  await click("选择提交说明语言"); await click("English");
  expect(localStorage.getItem(COMMIT_LANGUAGE_KEY)).toBe("en");
});
it("shows 提交 and 提交并推送 side by side", async () => {
  const labels = [...container.querySelectorAll(".review-commit-buttons > button")].map(button => button.textContent);
  expect(labels).toEqual(["提交", "提交并推送"]);
  expect(container.querySelector('[aria-label="选择提交操作"]')).toBeNull();
});
it("asks inline before staging all and commits only after confirmation", async () => {
  await type("first"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交");
  expect(invoke).toHaveBeenLastCalledWith("core_review_git_action", { taskId: "task", rootId: "repo", action: "commit", snapshotId: "snapshot", message: "first", stageAll: false });
  expect(container.querySelector("dialog")).toBeNull(); expect(changed).not.toHaveBeenCalled();
  await click("暂存全部并提交");
  expect(invoke).toHaveBeenLastCalledWith("core_review_git_action", { taskId: "task", rootId: "repo", action: "commit", snapshotId: "snapshot", message: "first", stageAll: true });
  expect(container.querySelector("input")!.value).toBe(""); expect(changed).toHaveBeenCalledOnce();
  expect(container.querySelector('.review-commit [role="status"]')).toBeNull();
});
it("dismisses the stage-all confirmation with Escape", async () => {
  await type("first"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交");
  const confirm = container.querySelector(".review-commit-confirm")!;
  expect(document.activeElement?.textContent).toBe("暂存全部并提交");
  await act(async () => { confirm.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
  expect(container.querySelector(".review-commit-confirm")).toBeNull();
  expect(invoke).toHaveBeenCalledOnce();
});
it("asks before staging and pushing when the preference is unset", async () => {
  await type("first"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交并推送");
  expect(invoke).toHaveBeenCalledOnce();
  expect(container.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(false);
  await click("暂存全部、提交并推送");
  expect(vi.mocked(invoke).mock.calls.map(([, args]) => args)).toEqual([
    expect.objectContaining({ action: "commit", stageAll: false }),
    expect.objectContaining({ action: "commit", stageAll: true, snapshotId: "snapshot" }),
    expect.objectContaining({ taskId: "task", operationId: expect.any(String) }),
  ]);
  expect(localStorage.getItem(SKIP_STAGE_ALL_CONFIRMATION_KEY)).toBeNull();
});
it.each(["提交", "提交并推送"])("remembers the checkbox across repositories and remounts for %s", async label => {
  await type("first"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交");
  await act(async () => container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
  expect(localStorage.getItem(SKIP_STAGE_ALL_CONFIRMATION_KEY)).toBeNull();
  await click("暂存全部并提交");
  expect(localStorage.getItem(SKIP_STAGE_ALL_CONFIRMATION_KEY)).toBe("true");
  await render(false, true, "other", true, "next-preview");
  await type("next"); vi.mocked(invoke).mockClear();
  await click(label);
  expect(invoke).toHaveBeenNthCalledWith(1, "core_review_git_action", {
    taskId: "task", rootId: "other", action: "commit", snapshotId: "next-preview", message: "next", stageAll: true,
  });
  expect(invoke).toHaveBeenCalledTimes(label === "提交" ? 1 : 2);
  if (label === "提交并推送") expect(invoke).toHaveBeenLastCalledWith("core_review_git_advance", expect.objectContaining({ taskId: "task", operationId: expect.any(String) }));
  expect(container.querySelector('[aria-label="确认暂存全部"]')).toBeNull();
  expect(container.querySelector("input")?.value).toBe("");
});
it.each(["cancel", "escape", "edit", "unmount"])("discards a checked stage-all preference after %s", async dismiss => {
  await type("first"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交并推送");
  await act(async () => container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
  if (dismiss === "cancel") await click("取消");
  else if (dismiss === "escape") await act(async () => container.querySelector('.review-commit-confirm')!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  else if (dismiss === "edit") await type("edited");
  else await render(false, true, "remounted");
  expect(localStorage.getItem(SKIP_STAGE_ALL_CONFIRMATION_KEY)).toBeNull();
  await type("next"); vi.mocked(invoke).mockClear().mockResolvedValueOnce(false);
  await click("提交");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_action", expect.objectContaining({ stageAll: false }));
  expect(container.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(false);
});
it("does not remember stage-all when the confirmed commit fails", async () => {
  await type("first"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交");
  await act(async () => container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
  vi.mocked(invoke).mockRejectedValueOnce("preview outdated");
  await click("暂存全部并提交");
  expect(localStorage.getItem(SKIP_STAGE_ALL_CONFIRMATION_KEY)).toBeNull();
  vi.mocked(invoke).mockClear().mockResolvedValueOnce(false);
  await click("提交");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_action", expect.objectContaining({ stageAll: false }));
});
it("keeps asking after the checkbox is unchecked", async () => {
  await type("first"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交");
  await act(async () => container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
  await act(async () => container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
  await click("取消");
  expect(container.querySelector('[aria-label="确认暂存全部"]')).toBeNull();
  expect(container.querySelector("input")?.value).toBe("first");
  vi.mocked(invoke).mockClear().mockResolvedValueOnce(false);
  await click("提交");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_action", expect.objectContaining({ stageAll: false }));
  expect(container.querySelector('[aria-label="确认暂存全部"]')).not.toBeNull();
});
it.each([false, true])("uses the saved preference for keyboard submit (push: %s)", async push => {
  localStorage.setItem(SKIP_STAGE_ALL_CONFIRMATION_KEY, "true");
  await render(false, true, "reopened"); await type("keyboard");
  await act(async () => container.querySelector<HTMLInputElement>('input[type="text"]')!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, shiftKey: push, bubbles: true })));
  expect(invoke).toHaveBeenNthCalledWith(1, "core_review_git_action", expect.objectContaining({ action: "commit", stageAll: true, snapshotId: "snapshot" }));
  expect(invoke).toHaveBeenCalledTimes(push ? 2 : 1);
  expect(container.querySelector('[aria-label="确认暂存全部"]')).toBeNull();
});
it("updates an already open panel when another window saves the preference", async () => {
  await act(async () => {
    localStorage.setItem(SKIP_STAGE_ALL_CONFIRMATION_KEY, "true");
    window.dispatchEvent(new StorageEvent("storage", { key: SKIP_STAGE_ALL_CONFIRMATION_KEY, newValue: "true" }));
  });
  expect(container.querySelector('.review-commit-buttons > button')?.getAttribute("data-tooltip")).toContain("自动暂存全部");
  await type("other window"); await click("提交并推送");
  expect(invoke).toHaveBeenNthCalledWith(1, "core_review_git_action", expect.objectContaining({ stageAll: true }));
  expect(container.querySelector('[aria-label="确认暂存全部"]')).toBeNull();
});
it("keeps the preference in memory when saving to storage fails", async () => {
  await type("first"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交");
  const save = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("Storage unavailable"); });
  try {
    await act(async () => container.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click());
    expect(container.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(true);
    await click("暂存全部并提交");
    await render(false, true, "other"); await type("second"); vi.mocked(invoke).mockClear();
    await click("提交");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("core_review_git_action", expect.objectContaining({ stageAll: true }));
    expect(container.querySelector('[aria-label="确认暂存全部"]')).toBeNull();
  } finally {
    save.mockRestore();
    await act(async () => setSkipStageAllConfirmation(false));
  }
});
it.each([false, true])("commits then pushes and retries only push after a remote failure (skip confirmation: %s)", async skip => {
  localStorage.setItem(SKIP_STAGE_ALL_CONFIRMATION_KEY, String(skip));
  await type("change");
  vi.mocked(invoke).mockResolvedValueOnce(true).mockRejectedValueOnce("远程不可用");
  await click("提交并推送");
  expect(container.textContent).toContain("提交已完成，推送失败");
  expect(container.querySelector("input")!.value).toBe("");
  await render(false, true, "repo", false);
  await click("重试推送");
  const actions = vi.mocked(invoke).mock.calls.filter(([name]) => name === "core_review_git_action" || name === "core_review_git_advance").map(([name, args]) => name === "core_review_git_advance" ? "push" : (args as { action: string }).action);
  expect(actions).toEqual(["commit", "push", "push"]);
  expect(container.querySelector(".review-commit")).toBeNull();
});
it("does not push when commit fails, and keeps the draft", async () => {
  await type("change");
  vi.mocked(invoke).mockRejectedValueOnce("pre-commit failed"); await click("提交并推送");
  expect(invoke).toHaveBeenCalledOnce(); expect(container.querySelector("input")!.value).toBe("change");
  expect(changed).toHaveBeenCalledOnce();
});
it("rejects duplicate submits while an operation is pending", async () => {
  await type("change"); let resolve!: (value: boolean) => void;
  vi.mocked(invoke).mockReturnValueOnce(new Promise(done => { resolve = done; }));
  await click("提交"); await click("处理中…"); expect(invoke).toHaveBeenCalledOnce();
  await act(async () => resolve(true)); expect(onBusy.mock.calls.map(([busy]) => busy)).toEqual([true, false]);
});
it("does not apply a late summary to another repository", async () => {
  await click("选择提交说明 Agent"); await click("Codex"); let resolve!: (value: string) => void;
  vi.mocked(invoke).mockReturnValueOnce(new Promise(done => { resolve = done; }));
  await click("生成提交说明"); await render(false, true, "other");
  await act(async () => resolve("old repo")); expect(container.querySelector("input")!.value).toBe("");
});
it("disables busy workspaces and hides non-Git or empty workspaces", async () => {
  await type("change"); await render(true);
  expect(container.querySelector<HTMLButtonElement>(".review-commit-buttons > button")!.disabled).toBe(true);
  await render(false, false); expect(container.querySelector(".review-commit")).toBeNull();
  await render(false, true, "repo", false); expect(container.querySelector(".review-commit")).toBeNull();
  expect(invoke).not.toHaveBeenCalled();
});

it("does not reuse a disabled saved Agent for summary generation", async () => {
  await click("选择提交说明 Agent"); await click("Codex");
  await act(async () => setAgentEnabled("codex", false));
  await click("生成提交说明");
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === "core_review_commit_summary")).toBe(false);
  expect(document.querySelector('.agent-picker-option[aria-label="Codex"]')).toBeNull();
  expect(container.textContent).toContain("暂无可用 Agent");
});
it("summarizes a non-fast-forward rejection and collapses its raw log", async () => {
  await type("change");
  vi.mocked(invoke).mockResolvedValueOnce(true).mockRejectedValueOnce("! [rejected] main -> main (fetch first)\nerror: failed to push some refs");
  await click("提交并推送");
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("远端有新提交");
  expect(container.querySelector('[role="alert"]')?.textContent).not.toContain("[rejected]");
  expect(container.querySelector("details")?.open).toBe(false);
  expect(container.querySelector("pre")?.textContent).toContain("fetch first");
  expect(container.querySelector("input")!.value).toBe("");
});
it.each([false, true])("refreshes a stale commit preview and retains the message without pushing (skip confirmation: %s)", async skip => {
  localStorage.setItem(SKIP_STAGE_ALL_CONFIRMATION_KEY, String(skip));
  await type("keep this draft");
  vi.mocked(invoke).mockRejectedValueOnce("REVIEW_STALE_COMMIT: changed HEAD");
  await click("提交并推送");
  expect(invoke).toHaveBeenCalledOnce(); expect(changed).toHaveBeenCalledOnce();
  expect(container.querySelector("input")?.value).toBe("keep this draft");
  expect(container.querySelector('[role="alert"]')?.textContent).toContain("可能已在其他地方提交");
  await render(false, true, "repo", false, "clean");
  expect(container.querySelector("input")).toBeNull();
  expect(container.querySelector('[role="alert"]')).not.toBeNull();
});
it("uses the preview originally confirmed before staging all even if a refresh finishes meanwhile", async () => {
  await type("message"); vi.mocked(invoke).mockResolvedValueOnce(false);
  await click("提交"); await render(false, true, "repo", true, "new-preview");
  await click("暂存全部并提交");
  expect(invoke).toHaveBeenLastCalledWith("core_review_git_action", expect.objectContaining({ snapshotId: "snapshot", stageAll: true }));
});

it("clears the previous push error after pull and clears retry state after a successful sync", async () => {
  await type("change"); vi.mocked(invoke).mockResolvedValueOnce(true).mockRejectedValueOnce("fetch first");
  await click("提交并推送"); expect(container.querySelector('[role="alert"]')).not.toBeNull();
  await render(false, true, "repo", false, "after-pull", 1, 0);
  expect(container.querySelector('[role="alert"]')).toBeNull(); expect(container.textContent).toContain("重试推送");
  await render(false, true, "repo", false, "after-sync", 2, 1);
  expect(container.querySelector(".review-commit")).toBeNull();
});
