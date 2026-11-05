import { expect, test } from "vitest";
import { findTerminalUrls, type TerminalRow } from "./terminalLinks";

const cols = 20;
const rows = (lines: Array<string | { text: string; wrapped: boolean }>) => {
  const built: TerminalRow[] = lines.map(line => {
    const { text, wrapped } = typeof line === "string" ? { text: line, wrapped: false } : line;
    return { text, wrapped, columns: [...text].map((_, index) => index) };
  });
  return (y: number) => built[y];
};
const wrapped = (text: string) => ({ text, wrapped: true });

test("a URL wrapped by the terminal is one link from any of its rows", () => {
  const row = rows(["https://example.com/", wrapped("aaaaaaaaaaaaaaaaaaaa"), wrapped("bbb done")]);
  const whole = [{ url: "https://example.com/aaaaaaaaaaaaaaaaaaaabbb", start: { x: 1, y: 1 }, end: { x: 3, y: 3 } }];
  expect(findTerminalUrls(row, 0, cols)).toEqual(whole);
  expect(findTerminalUrls(row, 1, cols)).toEqual(whole);
  expect(findTerminalUrls(row, 2, cols)).toEqual(whole);
});

test("a URL broken by a program's own layout is one link too", () => {
  const row = rows(["https://example.com/", "aaaaaaaaaaaaaaaaaaaa", "bbb"]);
  expect(findTerminalUrls(row, 2, cols)[0]).toEqual({ url: "https://example.com/aaaaaaaaaaaaaaaaaaaabbb", start: { x: 1, y: 1 }, end: { x: 3, y: 3 } });
});

test("a margin on every row isn't part of the URL", () => {
  const row = rows(["  https://example.c", "  om/aaaaaaaaaaaaaa", "  bbb"]);
  expect(findTerminalUrls(row, 1, cols)[0]).toEqual({ url: "https://example.com/aaaaaaaaaaaaaabbb", start: { x: 3, y: 1 }, end: { x: 5, y: 3 } });
});

test("a URL that stops short of the edge leaves the next row alone", () => {
  const row = rows(["https://a.test/x", "next line"]);
  expect(findTerminalUrls(row, 0, cols).map(link => link.url)).toEqual(["https://a.test/x"]);
  expect(findTerminalUrls(row, 1, cols)).toEqual([]);
});

test("blank cells at the end of a wrapped row still end the URL", () => {
  const row = rows(["https://a.test/x", wrapped("next")]);
  expect(findTerminalUrls(row, 0, cols).map(link => link.url)).toEqual(["https://a.test/x"]);
});

test("ordinary text reaching the edge doesn't join its neighbours", () => {
  const row = rows(["aaaaaaaaaaaaaaaaaaaa", "see https://a.test"]);
  expect(findTerminalUrls(row, 1, cols)).toEqual([{ url: "https://a.test", start: { x: 5, y: 2 }, end: { x: 18, y: 2 } }]);
  expect(findTerminalUrls(row, 0, cols)).toEqual([]);
});

test("punctuation around a URL stays outside it, brackets of its own stay in", () => {
  const url = (text: string) => findTerminalUrls(rows([text]), 0, 200)[0]!.url;
  expect(url("see https://a.test/x.")).toBe("https://a.test/x");
  expect(url("(https://a.test/x),")).toBe("https://a.test/x");
  expect(url("https://a.test/x_(y)")).toBe("https://a.test/x_(y)");
});

test("each URL on a row is its own link, in the cell it really occupies", () => {
  const row: TerminalRow = { text: "中 http://a.test http://b.test", columns: [], wrapped: false };
  row.columns = [0, 2, ...Array.from({ length: row.text.length - 2 }, (_, index) => index + 3)];
  const links = findTerminalUrls(y => y === 0 ? row : undefined, 0, 200);
  expect(links.map(link => link.url)).toEqual(["http://a.test", "http://b.test"]);
  expect(links[0]!.start.x).toBe(4);
});

test("a row outside the buffer has no links", () => {
  expect(findTerminalUrls(rows([]), 3, cols)).toEqual([]);
});
