// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, TimelineItem } from "@dogcodes/protocol";
import { ConversationContent } from "./ConversationActivity";

vi.mock("./TurnFiles", () => ({ TurnFiles: () => <div data-testid="turn-files" /> }));

let sequence = 0;
const item = (id: string, kind: string, text: string, turnId: `trn_${string}` = "trn_one"): TimelineItem => ({
  attachments: [], itemId: id, taskId: "tsk_test", turnId, kind, text, providerId: "kimi",
  sequence: ++sequence, occurredAt: "2026-11-10T10:00:00Z",
});
const tool = (id: string, command: string, status = "completed") =>
  item(id, "activity", JSON.stringify({ type: "tool", name: "Bash", status, input: { command } }));
const message = (id: string, text: string) => item(id, "assistant_message", text);
const thought = (id: string) => item(id, "activity", JSON.stringify({ type: "thought", status: "completed", durationMs: 7000 }));

let host: HTMLDivElement, root: ReturnType<typeof createRoot>;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  vi.unstubAllGlobals();
});
const render = (items: TimelineItem[], activeTurnId: string | null = "trn_one") => act(async () => root.render(
  <ConversationContent items={items} client={{} as CoreClient} activeTurnId={activeTurnId ?? undefined} onSubagent={() => {}}
    renderEntry={(i) => <p key={i.itemId} data-kind={i.kind}>{i.text}</p>} />,
));
const titles = () => [...host.querySelectorAll(".activity-title")]
  .filter((node) => !node.closest(".tool-run-toggle")).map((node) => node.textContent);
const toggles = () => [...host.querySelectorAll<HTMLButtonElement>(".tool-run-toggle")];

it("folds a run of calls to the latest one and counts the whole run", async () => {
  await render([tool("a", "ls"), tool("b", "npm run build"), tool("c", "npm test")]);
  // One row: the latest call on the left, the run's size on the right.
  expect(titles()).toEqual([]);
  const [toggle] = toggles();
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(toggle.querySelector(".activity-title")!.textContent).toBe("npm test");
  expect(toggle.querySelector(".tool-run-count")!.textContent).toBe("3 次工具调用");
  expect(toggle.querySelector(".tool-run-count")!.getAttribute("data-tooltip")).toBe("终端 3 次");
  expect(toggle.dataset.hiddenCalls).toBe("2");
  expect(host.querySelector(".tool-run-list")).toBeNull();
});

it("folds edit patches and artifact results into the same run", async () => {
  const edit = (id: string, before: string, after: string) => item(id, "activity", JSON.stringify({
    type: "tool", name: "Edit", status: "completed", input: { file_path: "main.ts", old_string: before, new_string: after },
  }));
  const captured = item("artifact", "activity", JSON.stringify({ type: "tool", artifact: { artifactId: "art_capture" } }));
  await render([tool("start", "pwd"), edit("first", "old\n", "new\nextra\n"), captured, edit("again", "other\n", "updated\n"), thought("last")]);
  expect(toggles()).toHaveLength(1);
  const [toggle] = toggles();
  expect(toggle.querySelector(".tool-run-count")?.textContent).toContain("4 次工具调用");
  expect(toggle.querySelector(".tool-run-edits")?.textContent).toContain("已编辑 1 个文件");
  expect(toggle.querySelector(".additions")?.textContent).toBe("+3");
  expect(toggle.querySelector(".deletions")?.textContent).toBe("−2");
  expect(host.querySelector(".inline-file-edit")).toBeNull();
  await act(async () => toggle.click());
  expect(host.querySelectorAll(".inline-file-edit")).toHaveLength(2);
  expect(host.querySelector('[data-testid="turn-files"]')).not.toBeNull();
});

it("does not report an attempted failed or running edit as edited files", async () => {
  await render(["failed", "running"].map((status, index) => item(String(index), "activity", JSON.stringify({
    type: "tool", name: "Edit", status, input: { path: "a.ts", old_string: "old", new_string: "new" },
  }))));
  expect(host.querySelector(".tool-run-edits")).toBeNull();
});

it("starts a new run only after assistant text", async () => {
  await render([
    tool("a", "ls"), tool("b", "pwd"),
    message("m", "接下来检查构建。"),
    tool("c", "npm run build"), tool("d", "npm test"),
    thought("t"),
    tool("e", "git status"),
  ]);
  // A thought joins the run around it; only text breaks one.
  expect(toggles()).toHaveLength(2);
  expect(toggles().map((toggle) => toggle.dataset.hiddenCalls)).toEqual(["1", "3"]);
  expect(toggles().map((toggle) => toggle.querySelector(".activity-title")!.textContent)).toEqual(["pwd", "git status"]);
  // Thoughts are not counted as tool calls.
  expect(toggles()[1].querySelector(".tool-run-count")!.textContent).toBe("3 次工具调用");
  expect(titles()).toEqual([]);
  expect(host.textContent).toContain("接下来检查构建。");
});

it("shows the current thought on the folded row while it runs", async () => {
  const live = item("t2", "activity", JSON.stringify({ type: "thought", status: "running", startedAt: new Date().toISOString(), liveText: "看看测试" }));
  await render([thought("t1"), tool("a", "ls"), live]);
  const [toggle] = toggles();
  expect(toggle.querySelector(".activity-label")!.textContent).toMatch(/^思考中/);
  expect(toggle.querySelector(".activity-title")).toBeNull();
  expect(toggle.classList.contains("activity-running")).toBe(true);
  expect(toggle.dataset.hiddenCalls).toBe("2");
  expect(toggle.querySelector(".tool-run-count")!.textContent).toBe("1 次工具调用");
  // The live preview shares the toggle's flex row instead of creating another row.
  expect(host.querySelector(".thought-line")).toBeNull();
  expect(toggle.querySelector(".thought-marquee")?.textContent).toBe("看看测试");
  expect(toggle.querySelector("[tabindex]")).toBeNull();
  await act(async () => toggle.click());
  expect(toggle.querySelector(".thought-marquee")).toBeNull();
  expect(host.querySelector(".tool-run-list .thought-line .thought-marquee")?.textContent).toBe("看看测试");
  await act(async () => toggle.click());
  expect(toggle.querySelector(".thought-marquee")?.textContent).toBe("看看测试");
});

it("folds computer-use actions with the other steps", async () => {
  const action = item("cu", "computer_use", JSON.stringify({ tool: "click", mode: "background" }));
  await render([thought("t"), action, tool("a", "ls")], null);
  const [toggle] = toggles();
  expect(toggle.dataset.hiddenCalls).toBe("2");
  expect(toggle.querySelector(".tool-run-count")!.getAttribute("data-tooltip")).toContain("终端 1");
  await act(async () => toggle.click());
  expect(host.querySelectorAll(".tool-run-list > *")).toHaveLength(3);
});

it("does not fold a single call or runs from different turns", async () => {
  await render([tool("a", "ls"), tool("b", "pwd", "completed"), { ...tool("c", "npm test"), turnId: "trn_two" }], "trn_two");
  expect(toggles()).toHaveLength(1);
  expect(toggles()[0].querySelector(".activity-title")!.textContent).toBe("pwd");
  expect(titles()).toEqual(["npm test"]);
});

it("keeps approval-waiting calls on screen and folds running and failed ones", async () => {
  await render([
    tool("a", "ls"),
    tool("b", "rm -rf build", "awaiting_approval"),
    tool("c", "npm run build", "failed"),
    tool("d", "git status"),
    tool("e", "npm test", "running"),
  ]);
  // The latest (running) call heads the row; the others stay listed under it.
  const [toggle] = toggles();
  expect(toggle.querySelector(".activity-title")!.textContent).toBe("npm test");
  expect(toggle.classList.contains("activity-running")).toBe(true);
  expect(titles()).toEqual(["rm -rf build"]);
  expect(toggle.dataset.hiddenCalls).toBe("3");
  expect(toggle.querySelector(".activity-status-failed")!.textContent).toContain("1 次失败");
  await act(async () => toggle.click());
  const failedRow = [...host.querySelectorAll(".activity-line")].find((row) => row.textContent?.includes("npm run build"))!;
  expect(failedRow.querySelector(".activity-status-failed")!.textContent).toBe("失败");
});

it("shows no fold when every call must stay visible", async () => {
  await render([tool("a", "rm -rf build", "awaiting_approval"), tool("b", "npm test", "pending_approval")]);
  expect(toggles()).toHaveLength(0);
  expect(titles()).toEqual(["rm -rf build", "npm test"]);
});

it("folds parallel calls immediately and keeps the same group while results stream in", async () => {
  const a = tool("a", "lsof -a -p 79338 -d cwd -Fn", "running");
  const b = item("b", "activity", JSON.stringify({ type: "tool", name: "Read", status: "in_progress", input: { file_path: "electron/main.ts" } }));
  const c = tool("c", "npm run build", "started");
  const complete = (call: TimelineItem) => ({ ...call, text: JSON.stringify({ ...JSON.parse(call.text), status: "completed" }) });
  await render([a, b]);
  expect(toggles()).toHaveLength(1);
  const toggle = toggles()[0];
  expect(toggle.querySelector(".tool-run-count")?.textContent).toBe("2 次工具调用");
  expect(titles()).toEqual([]);
  expect(host.querySelector(".tool-run-list")).toBeNull();
  for (const calls of [[a, b, c], [a, complete(b), complete(c)], [complete(a), complete(b), complete(c)]]) {
    await render(calls);
    expect(toggles()).toEqual([toggle]);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.querySelector(".tool-run-count")?.textContent).toBe("3 次工具调用");
    expect(titles()).toEqual([]);
    expect(host.querySelector(".tool-run-list")).toBeNull();
    expect(toggle.classList.contains("activity-running")).toBe(calls.some(call => JSON.parse(call.text).status !== "completed"));
  }
  await act(async () => toggle.click());
  expect(titles()).toEqual(["lsof -a -p 79338 -d cwd -Fn", "electron/main.ts", "npm run build"]);
});

it("folds legacy tool.started entries before they complete", async () => {
  await render([item("a", "tool.started", "read a"), item("b", "tool.started", "read b")]);
  expect(toggles()).toHaveLength(1);
  expect(toggles()[0].classList.contains("activity-running")).toBe(true);
  expect(toggles()[0].querySelector(".tool-run-count")?.textContent).toBe("2 次工具调用");
  expect(host.querySelector(".tool-run-list")).toBeNull();
  await act(async () => toggles()[0].click());
  expect(host.querySelectorAll('.tool-run-list [data-kind="tool.started"]')).toHaveLength(2);
});

it("keeps the latest approval visible when other calls are folded", async () => {
  await render([tool("a", "pwd", "running"), tool("b", "npm run build", "pending_approval")]);
  expect(toggles()).toHaveLength(1);
  expect(toggles()[0].querySelector(".activity-title")?.textContent).toBe("pwd");
  expect(titles()).toEqual(["npm run build"]);
  expect(host.querySelector(".tool-run-list .conversation-activity")).not.toBeNull();
  await render([tool("a", "pwd", "running"), tool("b", "npm run build", "completed")]);
  expect(host.querySelector(".tool-run-list")).toBeNull();
  expect(toggles()[0].classList.contains("activity-running")).toBe(true);
});

it("folds a failed call with the rest of its run", async () => {
  await render([tool("a", "cat AGENTS.md", "failed"), tool("b", "cat README.md")], null);
  expect(toggles()).toHaveLength(1);
  expect(titles()).toEqual([]);
  expect(toggles()[0].querySelector(".tool-run-count")!.textContent).toBe("1 次失败 · 2 次工具调用");
});

it("shows a lone failed call as its own row", async () => {
  await render([tool("a", "cat AGENTS.md", "failed")], null);
  expect(toggles()).toHaveLength(0);
  expect(titles()).toEqual(["cat AGENTS.md"]);
});

it("expands to the same rows the unfolded timeline renders", async () => {
  const run = [tool("a", "ls"), tool("b", "npm run build", "failed"), tool("c", "npm test")];
  await render(run.slice(0, 1));
  const single = host.querySelector(".conversation-activity")!.outerHTML;
  await render(run);
  await act(async () => toggles()[0].click());
  expect(toggles()[0].getAttribute("aria-expanded")).toBe("true");
  expect(titles()).toEqual(["ls", "npm run build", "npm test"]);
  expect(host.querySelectorAll(".tool-run-list > *")).toHaveLength(3);
  expect(host.querySelector(".conversation-activity")!.outerHTML).toBe(single);
  // Opening a row inside the run keeps working as before.
  const details = host.querySelector<HTMLDetailsElement>(".conversation-activity")!;
  await act(async () => details.querySelector("summary")!.click());
  expect(details.open).toBe(true);
  expect(details.querySelector(".terminal-activity-command")!.textContent).toContain("ls");
  await act(async () => toggles()[0].click());
  expect(titles()).toEqual([]);
});

it("folds history the same way once the turn is over", async () => {
  const completion = { startedAt: "2026-11-10T10:00:00Z", completedAt: "2026-11-10T10:01:00Z" };
  await render([
    { ...item("p", "user_message", "打包"), turnCompletion: completion },
    message("m1", "先检查。"),
    tool("a", "ls"), tool("b", "npm run build", "running"), tool("c", "npm test"),
    message("final", "完成了。"),
  ], null);
  const process = host.querySelector<HTMLDetailsElement>(".turn-process")!;
  await act(async () => process.querySelector("summary")!.click());
  await act(async () => process.dispatchEvent(new Event("toggle")));
  // A call left "running" by a finished turn is no longer in progress.
  expect(toggles()).toHaveLength(1);
  expect(toggles()[0].dataset.hiddenCalls).toBe("2");
  expect(toggles()[0].classList.contains("activity-running")).toBe(false);
  expect(titles()).toEqual([]);
});
