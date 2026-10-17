// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { expect, it, vi } from "vitest";
import { StreamingMarkdown } from "./streamingMarkdown";
import { MarkdownMessage } from "./MarkdownMessage";

const plugins = [remarkGfm, remarkMath];
const html = (text: string) => renderToStaticMarkup(<Markdown skipHtml remarkPlugins={plugins}>{text}</Markdown>).replace(/\n/g, "");
it("matches the full parser at every streamed character, including changing block boundaries", () => {
  const documents = [
    "hello\n#heading\n\n- first\n\n- second\n\n---\n\nend",
    "# Title\n\nparagraph\n\n1. one\n\n2. two\n   continuation\n\n> quote\n>\n> more\n\nend",
    "intro\n\nTitle\n===\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nnext",
    "start\n\n```ts\nconst x = 1\n\n# code\n```\n\n$$\nx^2\n$$\n\nlast",
    "[later][id]\n\nother\n\n[id]: https://example.com\n\nend",
    "foot[^a]\n\nother\n\n[^a]: note\n    continued\n\nend",
    "outside\n\n> [id]: https://example.com\n\n[link][id]\n\nend",
    "one\n\n<div>\n\ntext\n</div>\n\nend",
    "  first\n\n    indented code\n\n    continued\n\nend",
    "first\n\n- outer\n  - inner\n\n    more\n\n- next\n\nend",
    "first\n\n   ~~~~js\nconst a = 1;\n~~~\n~~~~\n\nend",
    "first\n\n> quote\nlazy continuation\n\nnext\n\nlast",
  ];
  for (const document of documents) {
    const stream = new StreamingMarkdown();
    for (let end = 1; end <= document.length; end++) {
      const text = document.slice(0, end);
      const actual = stream.update(text, true).map(block => html(block.text)).join("");
      expect(actual, JSON.stringify(text)).toBe(html(text));
    }
  }
});

it("retains stable blocks and resets for corrections or canonical completion", () => {
  const stream = new StreamingMarkdown();
  const first = stream.update("one\n\ntwo\n\nthree", true);
  const next = stream.update("one\n\ntwo\n\nthree more", true);
  expect(next[0]).toBe(first[0]);
  expect(next[0].pending).toBe(false);
  expect(stream.update("replacement", true).map(block => block.text).join("")).toBe("replacement");
  expect(stream.update("replacement\n\nend", false)).toEqual([{ offset: 0, text: "replacement\n\nend", pending: false }]);
});

it("keeps completed DOM and defers highlighting until the growing fence closes", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div"), root = createRoot(container);
  try {
    const prefix = "# Title\n\n```ts\nconst stable = 1;\n```\n\nparagraph\n\n";
    await act(async () => root.render(<MarkdownMessage text={prefix + "```ts\nconst growing ="} streaming />));
    const stable = container.querySelector(".code-block");
    expect(stable?.querySelector(".hljs-keyword")).not.toBeNull();
    expect(container.querySelectorAll(".code-block")[1].querySelector(".hljs-keyword")).toBeNull();
    await act(async () => root.render(<MarkdownMessage text={prefix + "```ts\nconst growing = 42;"} streaming />));
    expect(container.querySelector(".code-block")).toBe(stable);
    await act(async () => root.render(<MarkdownMessage text={prefix + "```ts\nconst growing = 42;\n```"} streaming />));
    expect(container.querySelectorAll(".code-block")[1].querySelector(".hljs-keyword")).not.toBeNull();
    await act(async () => root.render(<MarkdownMessage text={prefix + "```ts\nconst growing = 42;\n```"} />));
    expect(container.querySelectorAll(".code-block")).toHaveLength(2);
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});
