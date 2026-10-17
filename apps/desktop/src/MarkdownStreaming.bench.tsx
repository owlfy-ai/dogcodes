// @vitest-environment jsdom
// pnpm --filter @dogcodes/desktop exec vitest bench src/MarkdownStreaming.bench.tsx --run
import { bench, describe } from "vitest";
import { createRoot } from "react-dom/client";
import { flushSync } from "react-dom";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeHighlight from "rehype-highlight";
import { MarkdownMessage } from "./MarkdownMessage";

const prefix = Array.from({ length: 60 }, (_, i) =>
  `## Section ${i}\n\nA paragraph with **bold**, a [link](https://example.com) and more text.\n\n\`\`\`ts\nconst result${i} = items.map(item => item.value * 2);\n\`\`\`\n\n`).join("");
const updates = Array.from({ length: 40 }, (_, i) => prefix + "Tail text grows smoothly. ".repeat(i + 1));

function replay(incremental: boolean) {
  const node = document.createElement("div"), root = createRoot(node);
  try {
    // Include initial mount plus successive updates in both variants.
    const render = (text: string) => flushSync(() => root.render(incremental
      ? <MarkdownMessage text={text} streaming />
      : <Markdown skipHtml remarkPlugins={[remarkGfm, remarkMath]}
          rehypePlugins={[[rehypeHighlight, { detect: false, ignoreMissing: true }]]}>{text}</Markdown>));
    render(prefix);
    for (const text of updates) render(text);
  } finally { flushSync(() => root.unmount()); }
}
describe("60 sections + 40 streamed updates (jsdom, includes initial mount)", () => {
  const options = { iterations: 3, time: 0, warmupIterations: 1, warmupTime: 0 };
  bench("full message parsing and highlighting", () => replay(false), options);
  bench("stable blocks + incremental tail", () => replay(true), options);
});
