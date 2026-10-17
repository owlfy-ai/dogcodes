import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import type { CoreClient, TimelineItem } from "@dogcodes/protocol";
import { ConversationContent } from "./ConversationActivity";

it("attributes each historical response once, including turns without thoughts", () => {
  const item = (id: string, turn: `trn_${string}`, kind: string, providerId: string | null, text = "answer"): TimelineItem => ({attachments: [], 
    itemId: id, taskId: "tsk_test", turnId: turn, kind, providerId, text, sequence: 1, occurredAt: "2026-09-12T00:00:00Z",
  });
  const html = renderToStaticMarkup(createElement(ConversationContent, {
    client: {} as CoreClient, onSubagent: () => {},
    items: [
      item("u1", "trn_1", "user_message", null),
      item("s1", "trn_1", "activity", "cursor", JSON.stringify({ type: "status" })),
      item("t1", "trn_1", "activity", "cursor", JSON.stringify({ type: "thought", status: "completed", durationMs: 3000 })),
      item("a1", "trn_1", "assistant_message", "cursor"),
      item("u2", "trn_2", "user_message", null),
      item("a2", "trn_2", "assistant_message", "codex"),
      item("a3", "trn_2", "assistant_message", "codex"),
    ],
    renderEntry: i => createElement("p", { key: i.itemId }, i.text),
    renderAgent: provider => createElement("span", { "data-provider": provider }, provider),
  }));
  expect(html.match(/response-agent-badge/g)).toHaveLength(2);
  expect(html.match(/data-provider="cursor"/g)).toHaveLength(1);
  expect(html.match(/data-provider="codex"/g)).toHaveLength(1);
  expect(html.indexOf('data-provider="cursor"')).toBeLessThan(html.indexOf("思考"));
});
