// @vitest-environment jsdom
import { beforeEach, expect, it, vi } from "vitest";
import type { ModelOption } from "@dogcodes/protocol";
import { MODEL_PREFERENCES_KEY, initialModelChoice, readRememberedModel, rememberModel } from "./modelPreferences";

const option = (id: string, thinking: string[] = [], defaultThinking: string | null = null): ModelOption =>
  ({ id, label: id, thinkingOptions: thinking.map(item => ({ id: item, label: item })), defaultThinking });
const models = [option("first", ["high", "low"], "high"), option("second", ["high", "low"], "high"), option("third")];
beforeEach(() => { vi.restoreAllMocks(); localStorage.clear(); });

it("uses the first model until a choice is remembered, then the remembered one", () => {
  expect(initialModelChoice("codex", models)).toEqual({ model: "first", thinking: "high" });
  rememberModel("codex", "second", "low");
  expect(initialModelChoice("codex", models)).toEqual({ model: "second", thinking: "low" });
});

it("keeps a separate choice for each Agent", () => {
  rememberModel("codex", "second", null);
  rememberModel("claude-code", "third", null);
  expect(readRememberedModel("codex")?.model).toBe("second");
  expect(readRememberedModel("claude-code")?.model).toBe("third");
  expect(initialModelChoice("cursor", models)?.model).toBe("first");
});

it("falls back to the default when the remembered model or effort is no longer offered", () => {
  rememberModel("codex", "retired", "high");
  expect(initialModelChoice("codex", models)).toEqual({ model: "first", thinking: "high" });
  rememberModel("codex", "second", "max");
  expect(initialModelChoice("codex", models)).toEqual({ model: "second", thinking: "high" });
  expect(initialModelChoice("codex", [])).toBeNull();
});

it("ignores empty choices and malformed storage", () => {
  rememberModel("codex", null, null);
  expect(localStorage.getItem(MODEL_PREFERENCES_KEY)).toBeNull();
  localStorage.setItem(MODEL_PREFERENCES_KEY, "{not json");
  expect(initialModelChoice("codex", models)?.model).toBe("first");
  localStorage.setItem(MODEL_PREFERENCES_KEY, JSON.stringify({ codex: { model: 3 }, cursor: { model: "third" } }));
  expect(readRememberedModel("codex")).toBeNull();
  expect(readRememberedModel("cursor")).toEqual({ model: "third", thinking: null });
});

it("keeps working when storage throws", () => {
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
  rememberModel("codex", "third", null);
  expect(initialModelChoice("codex", models)?.model).toBe("third");
});
