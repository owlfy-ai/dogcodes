// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { TextReveal, useSmoothText } from "./useSmoothText";
import { StreamingMarkdownMessage } from "./MarkdownMessage";

it("spreads a burst across frames and catches a large backlog within 160ms", () => {
  const reveal = new TextReveal("前文");
  reveal.append("前文一二三四五六七八", 0);
  expect(reveal.frame(16)).toBe("前文一");
  expect(reveal.frame(32)).toBe("前文一二");
  reveal.append("前文一二三四五六七八" + "字".repeat(1000), 32);
  expect(reveal.frame(192)).toBe(reveal.text);
  expect(reveal.waiting).toBe(false);
});

it("preserves queued text through frequent chunks and does not split graphemes", () => {
  const reveal = new TextReveal("");
  reveal.append("你👨‍👩‍👧‍👦e\u0301", 0);
  expect(reveal.frame(12)).toBe("你");
  reveal.append("你👨‍👩‍👧‍👦e\u0301！", 13);
  expect(reveal.frame(24)).toBe("你👨‍👩‍👧‍👦");
  expect(reveal.frame(36)).toBe("你👨‍👩‍👧‍👦e\u0301");
  expect(reveal.frame(48)).toBe(reveal.text);
  reveal.append("修正后的内容", 50);
  expect(reveal.frame(50)).toBe("修正后的内容");
});

let root: Root, host: HTMLDivElement, now: number, frames: Map<number, FrameRequestCallback>, nextId: number;
function Probe({ text, streaming = true }: { text: string; streaming?: boolean }) {
  return <p>{useSmoothText(text, streaming)}</p>;
}
const render = async (text: string, streaming = true) => act(async () => root.render(<Probe text={text} streaming={streaming} />));
const frame = async (time: number) => act(async () => {
  now = time;
  const callbacks = [...frames.values()]; frames.clear();
  callbacks.forEach(callback => callback(now));
});
beforeEach(() => {
  now = 0; nextId = 0; frames = new Map();
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { frames.set(++nextId, callback); return nextId; });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id));
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  vi.spyOn(performance, "now").mockImplementation(() => now);
  vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  host = document.createElement("div"); root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  expect(frames.size).toBe(0);
  vi.restoreAllMocks(); vi.unstubAllGlobals();
});

it("shows loaded history immediately, smooths additions, then stops scheduling frames", async () => {
  await render("已有正文");
  expect(host.textContent).toBe("已有正文");
  expect(frames.size).toBe(0);
  await render("已有正文一二三四五六七八");
  expect(host.textContent).toBe("已有正文");
  await frame(16);
  expect(host.textContent).toBe("已有正文一");
  await frame(48);
  expect(host.textContent).toBe("已有正文一二三四");
  await frame(160);
  expect(host.textContent).toBe("已有正文一二三四五六七八");
  expect(frames.size).toBe(0);
});

it("flushes on completion, corrections and backgrounding without losing text", async () => {
  await render("A"); await render("ABCDEFG");
  await render("ABCDEFG", false);
  expect(host.textContent).toBe("ABCDEFG");
  expect(frames.size).toBe(0);
  await render("replacement");
  expect(host.textContent).toBe("replacement");
  await render("replacement extra");
  await act(async () => document.dispatchEvent(new Event("visibilitychange")));
  expect(host.textContent).toBe("replacement extra");
  expect(frames.size).toBe(0);
});

it("respects reduced motion", async () => {
  vi.stubGlobal("matchMedia", () => ({ matches: true, addEventListener() {}, removeEventListener() {} }));
  await render("A"); await render("ABCDEFG");
  expect(host.textContent).toBe("ABCDEFG");
  expect(frames.size).toBe(0);
});

it("cancels pending frames on unmount", async () => {
  await render("A"); await render("ABCDEFG");
  expect(frames.size).toBe(1);
  await act(async () => root.render(null));
  expect(frames.size).toBe(0);
});

it("renders the final Markdown canonically even when completion interrupts a burst", async () => {
  await act(async () => root.render(<StreamingMarkdownMessage text="# 标题\n\n" streaming />));
  await act(async () => root.render(<StreamingMarkdownMessage text="# 标题\n\n**完整正文**" streaming />));
  expect(host.querySelector("strong")).toBeNull();
  await act(async () => root.render(<StreamingMarkdownMessage text="# 标题\n\n**完整正文**" />));
  expect(host.querySelector("strong")?.textContent).toBe("完整正文");
  expect(frames.size).toBe(0);
});
