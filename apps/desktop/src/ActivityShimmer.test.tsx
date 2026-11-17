// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { CoreClient, TimelineItem } from "@dogcodes/protocol";
import { ConversationContent } from "./ConversationActivity";

// The shimmer is CSS on `.activity-running .activity-text`; these tests pin
// which rows carry that class while a turn streams.
vi.mock("./TurnFiles", () => ({ TurnFiles: () => <div data-testid="turn-files" /> }));

let sequence = 0;
const activity = (id: string, data: object): TimelineItem => ({
  attachments: [], itemId: id, taskId: "tsk_test", turnId: "trn_one", kind: "activity",
  text: JSON.stringify(data), providerId: "kimi", sequence: ++sequence, occurredAt: "2026-11-10T10:00:00Z",
});
const tool = (id: string, command: string, status: string) =>
  activity(id, { type: "tool", name: "Bash", status, input: { command } });
const message = (id: string, text: string) => activity(id, { type: "message", text });

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
const render = (items: TimelineItem[], activeTurnId?: string) => act(async () => root.render(
  <ConversationContent items={items} client={{} as CoreClient} activeTurnId={activeTurnId} onSubagent={() => {}}
    renderEntry={(i) => <p key={i.itemId}>{i.text}</p>} />,
));
const running = () => [...host.querySelectorAll(".activity-running")];

it("shimmers only the thought that is still running", async () => {
  await render([
    activity("t1", { type: "thought", status: "completed", durationMs: 4000, text: "earlier" }),
    message("m", "我会再创建一个临时测试文件。"),
    activity("t2", { type: "thought", status: "running", startedAt: new Date().toISOString() }),
  ], "trn_one");
  const [row] = running();
  expect(running()).toHaveLength(1);
  expect(row.classList.contains("thought-line")).toBe(true);
  // The label text carries the shimmer, not the row background.
  expect(row.querySelector(".activity-text")!.textContent).toMatch(/^思考中/);
  expect(host.querySelector(".thought-history")!.contains(row)).toBe(false);
});

it("shimmers a running tool row and stops once it completes, fails or is cancelled", async () => {
  const touch = "touch /tmp/kimi-manual-approval-demo.txt";
  await render([message("m", "开始。"), tool("c", touch, "in_progress")], "trn_one");
  const [row] = running();
  expect(running()).toHaveLength(1);
  expect(row.matches("summary.activity-line")).toBe(true);
  expect([...row.querySelectorAll(".activity-text")].map((node) => node.textContent)).toContain(touch);
  for (const status of ["completed", "failed", "cancelled"]) {
    await render([message("m", "开始。"), tool("c", touch, status)], "trn_one");
    expect(running()).toEqual([]);
  }
});

it("does not shimmer rows of a turn that is no longer streaming", async () => {
  await render([
    activity("t", { type: "thought", status: "running", startedAt: new Date().toISOString() }),
    message("m", "开始。"),
    tool("c", "npm test", "running"),
  ]);
  expect(running()).toEqual([]);
});
