// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { beforeEach, afterEach, it, expect, vi } from "vitest";
import { MarkdownMessage } from "./MarkdownMessage";
import {
  ActivityRow,
  SubagentPanel,
  belongsTo,
  type Activity,
} from "./ConversationActivity";
import { mathSyntax } from "./mathSyntax";
import { editPatch, frozenDiff } from "./conversationDiff";
import { parseGitDiff } from "./diffView";
import type { TimelineItem } from "@dogcodes/protocol";
vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(async () => ({ svg: "<svg><text>diagram</text></svg>" })),
  },
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => {
    if (command === "core_workspace_file_applications") {
      return { absolutePath: "/project/src/example.ts", applications: [], error: null };
    }
    throw new Error(`Unexpected native command: ${command}`);
  }),
}));
let root: Root, container: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.unstubAllGlobals();
});
const item = (id: string, data: Activity): TimelineItem => ({attachments: [], 
  itemId: `tool:tsk_a:trn_a:${id}`,
  taskId: "tsk_a",
  turnId: "trn_a",
  kind: "activity",
  text: JSON.stringify(data),
  sequence: 1,
  occurredAt: "2026-09-11T00:00:00Z",
  providerId: "claude-code",
});
it("renders GFM, highlighted code, formulas without raw HTML execution", async () => {
  await act(async () =>
    root.render(
      <MarkdownMessage
        text={
          "## 标题\n\n| 名称 | 值 |\n| --- | --- |\n| A | 42 |\n\n$x^2$\n\n```ts\nconst n = 42;\n```\n\n<script>alert(1)</script>\n[bad](javascript:alert%281%29)"
        }
      />,
    ),
  );
  expect(container.querySelector("h2")?.textContent).toBe("标题");
  expect(container.querySelector("table")).not.toBeNull();
  expect(container.querySelector(".katex")).not.toBeNull();
  expect(
    container.querySelector(".katex .vlist")?.getAttribute("style"),
  ).toContain("height");
  expect(container.querySelector(".hljs-keyword")).not.toBeNull();
  expect(container.querySelector("script")).toBeNull();
  expect(container.querySelector('a[href^="javascript:"]')).toBeNull();
});
it("waits for a closed Mermaid fence, then renders and offers source", async () => {
  await act(async () =>
    root.render(
      <MarkdownMessage text={"```mermaid\ngraph TD\nA --> B"} streaming />,
    ),
  );
  expect(container.textContent).toContain("图表生成中");
  expect(container.querySelector("svg")).toBeNull();
  await act(async () =>
    root.render(
      <MarkdownMessage text={"```mermaid\ngraph TD\nA --> B\n```"} />,
    ),
  );
  expect(container.querySelector("svg")).not.toBeNull();
});
it("preserves code dollars and currency while normalizing TeX delimiters", () => {
  expect(mathSyntax("费用 $5 与 $10，公式 \\(x^2\\) 和 `$y$`")).toContain(
    "费用 \\$5",
  );
  expect(mathSyntax("\\[x^2\\]")).toBe("$$x^2$$");
  expect(mathSyntax("```tex\n\\(x\\)\n```")).toBe("```tex\n\\(x\\)\n```");
});
it("routes explicit child records to the correct parent and turn", () => {
  const parent = item("task1", { type: "subagent", childId: "child1" });
  expect(
    belongsTo(item("read", { type: "tool", parentId: "task1" }), parent),
  ).toBe(true);
  expect(
    belongsTo(item("read", { type: "tool", parentId: "task2" }), parent),
  ).toBe(false);
  expect(
    belongsTo(
      {
        ...item("read", { type: "tool", parentId: "task1" }),
        turnId: "trn_other",
      },
      parent,
    ),
  ).toBe(false);
});
it("shows unsupported detail explicitly instead of an empty loading state", async () => {
  const parent = item("task1", {
    type: "subagent",
    details: "unavailable",
    status: "completed",
  });
  await act(async () =>
    root.render(
      <SubagentPanel item={parent} items={[parent]} onClose={() => {}} />,
    ),
  );
  expect(container.textContent).toContain(
    "当前 Agent 无法查看子智能体详细信息",
  );
});
it("never displays thought text after completion or in inactive historical turns", async () => {
  const a: Activity = {
    type: "thought",
    status: "completed",
    durationMs: 3100,
    liveText: "discarded text",
  };
  await act(async () =>
    root.render(
      <ActivityRow
        item={item("t", a)}
        a={a}
        active={false}
        onSubagent={() => {}}
      />,
    ),
  );
  expect(container.textContent).toContain("3 秒");
  expect(container.textContent).not.toContain("discarded text");
});
it("shows a finished thought's saved text collapsed, never its live ticker", async () => {
  const a: Activity = {
    type: "thought",
    status: "completed",
    durationMs: 3100,
    text: "先读配置，再改测试。",
    textTruncated: true,
    liveText: "discarded text",
  };
  await act(async () =>
    root.render(
      <ActivityRow item={item("t", a)} a={a} active={false} onSubagent={() => {}} />,
    ),
  );
  const details = container.querySelector("details.thought-history");
  expect(details).not.toBeNull();
  expect((details as HTMLDetailsElement).open).toBe(false);
  expect(details!.querySelector("summary")!.textContent).toContain("3 秒");
  expect(details!.textContent).toContain("先读配置，再改测试。");
  expect(details!.textContent).toContain("只保存了前面一部分");
  expect(container.textContent).not.toContain("discarded text");
});
it("parses per-edit patches and computes net file statistics from frozen bytes", () => {
  const patch = editPatch("a.ts", "one\ntwo\n", "one\nthree\n");
  const parsed = parseGitDiff(new TextEncoder().encode(patch));
  expect(parsed.kind).toBe("structured");
  if (parsed.kind === "structured") {
    expect(parsed.diff.additions).toBe(1);
    expect(parsed.diff.deletions).toBe(1);
  }
  const hex = (s: string) =>
    Array.from(new TextEncoder().encode(s), (n) =>
      n.toString(16).padStart(2, "0"),
    ).join("");
  const net = frozenDiff({
    files: [
      {
        path: "a.ts",
        before: { hex: hex("one\ntwo\n") },
        after: { hex: hex("one\nthree\n") },
      },
    ],
  });
  expect(net.kind).toBe("structured");
  if (net.kind === "structured") expect(net.diff.additions).toBe(1);
});
it("does not redraw a completed diagram when following text streams in", async () => {
  const mermaid = (await import("mermaid")).default;
  vi.mocked(mermaid.render).mockClear();
  const diagram = "```mermaid\ngraph TD\nA --> B\n```";
  await act(async () =>
    root.render(<MarkdownMessage text={diagram} streaming />),
  );
  expect(mermaid.render).toHaveBeenCalledTimes(1);
  await act(async () =>
    root.render(
      <MarkdownMessage text={diagram + "\n\n下一段正在生成"} streaming />,
    ),
  );
  expect(mermaid.render).toHaveBeenCalledTimes(1);
});
it("keeps an unfinished display formula readable until its delimiter closes", async () => {
  await act(async () =>
    root.render(<MarkdownMessage text={"$$\nx^2"} streaming />),
  );
  expect(container.querySelector(".pending-math")).not.toBeNull();
  expect(container.querySelector(".katex")).toBeNull();
  await act(async () =>
    root.render(<MarkdownMessage text={"$$\nx^2\n$$"} streaming />),
  );
  expect(container.querySelector(".katex")).not.toBeNull();
});
it("loads a collapsed turn summary and opens frozen history instead of the live sidebar", async () => {
  const { webcrypto } = await import("node:crypto");
  vi.stubGlobal("crypto", webcrypto);
  const { TurnFiles } = await import("./TurnFiles");
  const patch = editPatch("src/example.ts", "before\n", "after\n");
  const bytes = new TextEncoder().encode(
    JSON.stringify({ files: [{ path: "src/example.ts", status: "M" }], patch }),
  );
  const hash = Array.from(
    new Uint8Array(await webcrypto.subtle.digest("SHA-256", bytes)),
    (n) => n.toString(16).padStart(2, "0"),
  ).join("");
  const metadata = {
    artifactId: "art_test" as const,
    payloadSha256: hash,
    byteLength: bytes.length,
    mediaType: "application/json",
    createdAt: "2026-09-11T00:00:00Z",
    redactionRulesetVersion: 0,
    redactionCount: 0,
  };
  const client = {
    readArtifact: vi.fn(async () => ({
      metadata,
      offset: 0,
      bytes: Array.from(bytes),
      nextOffset: null,
    })),
  } as unknown as import("@dogcodes/protocol").CoreClient;
  const onReviewFile = vi.fn();
  await act(async () =>
    root.render(
      <TurnFiles taskId="tsk_a" metadata={metadata} client={client} onReviewFile={onReviewFile} />,
    ),
  );
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20));
  });
  expect(container.textContent).toContain("1 个文件已更改");
  expect(container.textContent).toContain("+1");
  const summary = container.querySelector<HTMLDetailsElement>(".turn-summary")!;
  expect(summary.open).toBe(false);
  await act(async () => container.querySelector<HTMLElement>(".turn-summary-heading")!.click());
  expect(summary.open).toBe(true);
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="查看 example.ts 的历史 Diff"]')!.click());
  expect(onReviewFile).not.toHaveBeenCalled();
  expect(container.querySelector(".historical-turn-diff .diff-view")).not.toBeNull();
  await act(async () => root.render(<TurnFiles taskId="tsk_a" metadata={metadata} client={client} operation />));
  expect(container.querySelector(".turn-files")).toBeNull();
  expect(container.querySelector(".diff-view")).toBeNull();
  expect(container.querySelector(".inline-edit-heading")?.textContent).toContain("example.ts");
  expect(container.querySelector(".inline-edit-code")).toBeNull();
  await act(async () => {
    const details = container.querySelector<HTMLDetailsElement>(".inline-file-edit")!;
    details.open = true; details.dispatchEvent(new Event("toggle"));
  });
  expect(container.querySelector(".inline-edit-line.deletion")?.textContent).toContain("before");
  expect(container.querySelector(".inline-edit-line.addition")?.textContent).toContain("after");
});
