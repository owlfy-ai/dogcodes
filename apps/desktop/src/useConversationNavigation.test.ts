import { expect, it } from "vitest";
import { visitConversation, conversationNavigationIndex, type ConversationVisits } from "./useConversationNavigation";
const visits = (...ids: string[]) => ids.reduce(visitConversation, { entries: [], index: -1 } as ConversationVisits);
it("navigates visit order rather than task creation order and preserves forward history", () => {
  const history = visits("c", "a", "b");
  const available = new Set(["a", "b", "c"]);
  const back = conversationNavigationIndex(history, "b", -1, available)!;
  expect(history.entries[back]).toBe("a");
  expect(conversationNavigationIndex({ ...history, index: back }, "a", 1, available)).toBe(2);
  expect(conversationNavigationIndex(history, "b", 1, available)).toBeNull();
  expect(conversationNavigationIndex({ ...history, index: 0 }, "c", -1, available)).toBeNull();
});
it("avoids consecutive duplicates and clears forward history on a new visit", () => {
  const history = visits("a", "b", "c");
  expect(visitConversation(history, "c")).toBe(history);
  expect(visitConversation({ ...history, index: 1 }, "d")).toEqual({ entries: ["a", "b", "d"], index: 2 });
});
it("skips removed conversations and returns from an unsent draft", () => {
  const history = visits("a", "deleted", "c");
  const available = new Set(["a", "c"]);
  expect(conversationNavigationIndex(history, "c", -1, available)).toBe(0);
  expect(conversationNavigationIndex(history, null, -1, available)).toBe(2);
  expect(conversationNavigationIndex(history, null, 1, available)).toBeNull();
});
it("bounds the history while allowing repeat visits separated by other conversations", () => {
  expect(visits("a", "b", "a").entries).toEqual(["a", "b", "a"]);
  const history = visits(...Array.from({ length: 120 }, (_, index) => String(index)));
  expect(history.entries).toHaveLength(100);
  expect(history.entries[history.index]).toBe("119");
});
