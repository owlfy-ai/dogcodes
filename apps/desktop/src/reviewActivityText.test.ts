import { expect, it } from "vitest";
import { reviewActivityText } from "./reviewActivityText";

it("hides fenced, bare and unfinished JSON replies throughout streaming", () => {
  for (const reply of ['{"summary":"完成","findings":[]}', '```json\n{"summary":"完成","findings":[]}\n```', '~~~JSON\n{"findings":[]}\n~~~', '```\n{"findings":[]}\n```', '[{"title":"问题"}]']) {
    for (let length = 1; length <= reply.length; length++) {
      expect(reviewActivityText(reply.slice(0, length)), `${reply}: ${length}`).toBe("");
    }
  }
});
it("preserves progress, prose surrounding fenced JSON, and ordinary code and links", () => {
  expect(reviewActivityText('正在检查调用方。\n```json\n{"findings":[]}\n```\n检查完成。')).toBe('正在检查调用方。\n检查完成。');
  expect(reviewActivityText('正在整理结果。\n{\n  "summary": "未完成')).toBe('正在整理结果。');
  const markdown = '检查 [调用方](src/app.ts)。\n```ts\n{ value: 1 }\n```\n保留这段解释。';
  expect(reviewActivityText(markdown)).toBe(markdown);
});
it("keeps transport diagnostics out of the transcript without hiding quoted code evidence", () => {
  const error = 'Error: RetriableError: [internal] HTTP/2 keepalive ping timed out after 5000ms';
  expect(reviewActivityText(`正在核对调用方。\n\n${error}`)).toBe('正在核对调用方。');
  const evidence = `检查错误处理：\n\x60\x60\x60text\n${error}\n\x60\x60\x60`;
  expect(reviewActivityText(evidence)).toBe(evidence);
});
