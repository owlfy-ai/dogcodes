// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ReviewText } from "./ReviewText";
import type { TextComparison } from "./reviewClient";

let root: Root, container: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
const changed: TextComparison = {
  before: ["old\n", "second"], after: ["new\n"], additions: 1, deletions: 2,
  blocks: [{ kind: "replace", oldStart: 0, oldEnd: 2, newStart: 0, newEnd: 1 }],
};
it("anchors the visible original line when a refresh inserts rows above it", async () => {
  container.className = "review-file-body";
  const lines = ["a\n", "b\n", "c\n", "d\n"];
  const diff: TextComparison = { before: lines, after: lines, additions: 0, deletions: 0,
    blocks: [{ kind: "equal", oldStart: 0, oldEnd: 4, newStart: 0, newEnd: 4 }] };
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const rows = [...container.querySelectorAll(".review-inline-row")];
    const top = this === container ? 0 : rows.indexOf(this) * 20 - container.scrollTop;
    return { top, bottom: top + (this === container ? 40 : 20), height: 20, width: 100, left: 0, right: 100, x: 0, y: top, toJSON() {} };
  });
  await act(async () => root.render(<ReviewText diff={diff} split={false} />));
  container.scrollTop = 40;
  const updated: TextComparison = { ...diff, after: ["inserted\n", ...lines], additions: 1,
    blocks: [{ kind: "insert", oldStart: 0, oldEnd: 0, newStart: 0, newEnd: 1 },
      { kind: "equal", oldStart: 0, oldEnd: 4, newStart: 1, newEnd: 5 }] };
  await act(async () => root.render(<ReviewText diff={updated} split={false} />));
  expect(container.scrollTop).toBe(60);
  expect(container.querySelector('[data-review-old-line="2"]')!.getBoundingClientRect().top).toBe(0);
  await act(async () => root.render(<ReviewText diff={diff} split={false} />));
  expect(container.scrollTop).toBe(40);
});
it("renders inline deletion groups before additions and missing newline markers", async () => {
  await act(async () => root.render(<ReviewText diff={changed} split={false} />));
  expect([...container.querySelectorAll("code")].map(e => e.textContent)).toEqual(["old", "second", "new"]);
  expect(container.querySelectorAll(".review-removed")).toHaveLength(2);
  expect(container.querySelectorAll(".review-added")).toHaveLength(1);
  expect(container.querySelectorAll(".review-no-newline")).toHaveLength(1);
});

it.each([false, true])("preserves pure-added reading position across insertion and removal (split=%s)", async split => {
  container.className = "review-file-body";
  const lines = Array.from({ length: 20 }, (_, i) => `added ${i}\n`);
  const added = (after: string[]): TextComparison => ({ before: [], after, additions: after.length, deletions: 0,
    blocks: [{ kind: "insert", oldStart: 0, oldEnd: 0, newStart: 0, newEnd: after.length }] });
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const rows = [...container.querySelectorAll("[data-review-new-line]")];
    const top = this === container ? 0 : rows.indexOf(this) * 20 - container.scrollTop;
    return { top, bottom: top + (this === container ? 40 : 20), height: 20, width: 100, left: 0, right: 100, x: 0, y: top, toJSON() {} };
  });
  await act(async () => root.render(<ReviewText diff={added(lines)} split={split} />));
  container.scrollTop = 200;
  await act(async () => root.render(<ReviewText diff={added(["new prefix\n", ...lines])} split={split} />));
  expect(container.scrollTop).toBe(220);
  expect(container.querySelector('[data-review-new-line="11"]')!.textContent).toContain("added 10");
  await act(async () => root.render(<ReviewText diff={added(lines)} split={split} />));
  expect(container.scrollTop).toBe(200);
});
it("aligns unequal replacement lengths with an empty split-side cell", async () => {
  await act(async () => root.render(<ReviewText diff={changed} split />));
  const rows = container.querySelectorAll(".review-split-row");
  expect(rows).toHaveLength(2);
  expect(rows[0].querySelectorAll("code")).toHaveLength(2);
  expect(rows[1].children[1].textContent).toBe("");
});
it("expands unchanged context without losing it when switching layout", async () => {
  const lines = Array.from({ length: 20 }, (_, i) => `line ${i + 1}\n`);
  const diff: TextComparison = { before: lines, after: lines, additions: 0, deletions: 0,
    blocks: [{ kind: "equal", oldStart: 0, oldEnd: 20, newStart: 0, newEnd: 20 }] };
  await act(async () => root.render(<ReviewText diff={diff} split={false} />));
  expect(container.querySelectorAll("code")).toHaveLength(6);
  expect(container.querySelector("button")!.textContent).toContain("14");
  await act(async () => container.querySelector("button")!.click());
  expect(container.querySelectorAll("code")).toHaveLength(20);
  await act(async () => root.render(<ReviewText diff={diff} split />));
  expect(container.querySelectorAll("code")).toHaveLength(40);
});

it("retains opened context after new edits split and shift an equal block", async () => {
  const lines = Array.from({ length: 50 }, (_, i) => `line ${i}\n`);
  const before: TextComparison = { before: lines, after: lines, additions: 0, deletions: 0,
    blocks: [{ kind: "equal", oldStart: 0, oldEnd: 30, newStart: 0, newEnd: 30 },
      { kind: "equal", oldStart: 30, oldEnd: 50, newStart: 30, newEnd: 50 }] };
  await act(async () => root.render(<ReviewText diff={before} split={false} />));
  await act(async () => container.querySelector("button")!.click());
  const after = [...lines.slice(0, 15), "inserted\n", ...lines.slice(15)];
  const updated: TextComparison = { ...before, after, additions: 1,
    blocks: [{ kind: "equal", oldStart: 0, oldEnd: 15, newStart: 0, newEnd: 15 },
      { kind: "insert", oldStart: 15, oldEnd: 15, newStart: 15, newEnd: 16 },
      { kind: "equal", oldStart: 15, oldEnd: 30, newStart: 16, newEnd: 31 },
      { kind: "equal", oldStart: 30, oldEnd: 50, newStart: 31, newEnd: 51 }] };
  await act(async () => root.render(<ReviewText diff={updated} split={false} />));
  expect(container.querySelectorAll("code")).toHaveLength(37);
  expect(container.querySelectorAll(".review-context")).toHaveLength(1);
  expect(container.textContent).toContain("line 20");
  expect(container.textContent).toContain("inserted");
});

it.each([false, true])("highlights both versions with multiline context (split=%s)", async split => {
  const before = ['/* comment\n', 'continued */\n', 'const value = "old";\n'];
  const after = ['/* comment\n', 'continued */\n', 'const value = "<script>";\n'];
  const diff: TextComparison = { before, after, additions: 3, deletions: 3,
    blocks: [{ kind: "replace", oldStart: 0, oldEnd: 3, newStart: 0, newEnd: 3 }] };
  await act(async () => root.render(<ReviewText diff={diff} split={split} path="src/example.ts" />));
  expect(container.querySelectorAll('.hljs-keyword')).toHaveLength(2);
  expect(container.querySelector('[data-review-old-line="1"] .hljs-comment')?.textContent).toBe('continued */');
  expect(container.querySelector(split ? '[data-review-new-line="2"] > div:last-child .hljs-string' : '[data-review-new-line="2"] .hljs-string')?.textContent).toBe('"<script>"');
  expect(container.querySelector('script')).toBeNull();
  await act(async () => root.render(<ReviewText diff={diff} split={split} path="example.unknown" />));
  expect(container.querySelector('.hljs-keyword')).toBeNull();
  expect(container.textContent).toContain('const value = "<script>";');
});
