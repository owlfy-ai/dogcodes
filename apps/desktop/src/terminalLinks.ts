import { invoke, isTauri } from "@tauri-apps/api/core";
import type { IBufferCellPosition, IDisposable, ILinkProvider, Terminal } from "@xterm/xterm";

/** A row of the buffer. `columns[i]` is the cell that holds `text[i]`, as wide characters take two. */
export interface TerminalRow {
  text: string;
  columns: number[];
  /** The terminal itself continued the previous row here. */
  wrapped: boolean;
}

export interface TerminalUrl {
  url: string;
  start: IBufferCellPosition;
  end: IBufferCellPosition;
}

const URL_CHARS = "A-Za-z0-9\\-._~:/?#\\[\\]@!$&()*+,;=%";
const URL_PATTERN = new RegExp(`https?://[${URL_CHARS}]+`, "g");
const URL_AT_END = new RegExp(`https?://[${URL_CHARS}]*$`);
const URL_CHAR = new RegExp(`[${URL_CHARS}]`);
/** One URL can't take more rows than this; it also bounds the work per hover. */
const MAX_ROWS = 64;
const MAX_INDENT = 8;

const indentOf = (row: TerminalRow) => row.text.length - row.text.trimStart().length;
const lastColumn = (row: TerminalRow) => row.columns[row.text.length - 1] ?? -1;

/** Programs that lay out their own text break a long URL at the width they draw in,
 * which can be narrower than the window by their margin on either side. */
function reachesEdge(row: TerminalRow, cols: number): boolean {
  return lastColumn(row) >= cols - 1 - Math.min(indentOf(row), MAX_INDENT);
}

function withoutTrailingPunctuation(url: string): string {
  let end = url.length;
  for (; end > 0; end--) {
    const char = url[end - 1]!;
    if (".,;:!?".includes(char)) continue;
    const open = { ")": "(", "]": "[" }[char];
    if (!open) break;
    const body = url.slice(0, end);
    // Keep a bracket that belongs to the URL, drop one that surrounds it.
    if (body.split(open).length >= body.split(char).length) break;
  }
  return url.slice(0, end);
}

/** The URLs that touch row `y`, each whole however many rows it spans. `y` is 0-based. */
export function findTerminalUrls(row: (y: number) => TerminalRow | undefined, y: number, cols: number): TerminalUrl[] {
  const here = row(y);
  if (!here) return [];
  // Where the block of rows that could run into this one begins.
  let first = y;
  while (y - first < MAX_ROWS) {
    const current = row(first), previous = row(first - 1);
    if (!current || !previous || !(current.wrapped || reachesEdge(previous, cols))) break;
    first--;
  }

  let text = "";
  let cells: Array<{ y: number; x: number }> = [];
  for (let at = first, carried = false; ; at++) {
    const current = row(at)!;
    // A row that continues a URL by layout may repeat its margin, which isn't part of it.
    const skip = carried ? indentOf(current) : 0;
    text += current.text.slice(skip);
    for (let i = skip; i < current.text.length; i++) cells.push({ y: at, x: current.columns[i]! });

    const next = at - first < MAX_ROWS ? row(at + 1) : undefined;
    const nextWrapped = !!next?.wrapped;
    const nextCarried = !!next && !nextWrapped && reachesEdge(current, cols) && URL_AT_END.test(text)
      && URL_CHAR.test(next.text.trimStart()[0] ?? " ");
    if (nextWrapped || nextCarried) {
      // Blank cells at the end of a wrapped row still separate words.
      if (nextWrapped && lastColumn(current) < cols - 1) { text += " "; cells.push({ y: at, x: cols - 1 }); }
      carried = nextCarried;
      continue;
    }
    if (at >= y) break;
    text = ""; cells = []; carried = false;
  }

  const urls: TerminalUrl[] = [];
  for (const match of text.matchAll(URL_PATTERN)) {
    const url = withoutTrailingPunctuation(match[0]);
    const start = cells[match.index]!, end = cells[match.index + url.length - 1]!;
    if (start.y > y || end.y < y) continue;
    urls.push({ url, start: { x: start.x + 1, y: start.y + 1 }, end: { x: end.x + 1, y: end.y + 1 } });
  }
  return urls;
}

/** The URL at a cell, in the 1-based coordinates of `TerminalUrl`. */
export function terminalUrlAtCell(urls: TerminalUrl[], x: number, y: number): string | null {
  const after = (a: IBufferCellPosition, b: { x: number; y: number }) => a.y < b.y || a.y === b.y && a.x <= b.x;
  return urls.find(({ start, end }) => after(start, { x, y }) && after({ x, y }, end))?.url ?? null;
}

function bufferRow(terminal: Terminal, y: number): TerminalRow | undefined {
  const line = terminal.buffer.active.getLine(y);
  if (!line) return undefined;
  let text = "";
  const columns: number[] = [];
  for (let x = 0; x < line.length; x++) {
    const cell = line.getCell(x);
    if (!cell || cell.getWidth() === 0) continue;
    const chars = cell.getChars() || " ";
    text += chars;
    for (let i = 0; i < chars.length; i++) columns.push(x);
  }
  text = text.trimEnd();
  return { text, columns: columns.slice(0, text.length), wrapped: line.isWrapped };
}

/** The URL under a point on screen, for a menu about it. */
export function terminalUrlAtPoint(terminal: Terminal, clientX: number, clientY: number): string | null {
  const rect = terminal.element?.querySelector(".xterm-screen")?.getBoundingClientRect();
  if (!rect?.width || !rect.height) return null;
  const x = Math.floor((clientX - rect.left) / (rect.width / terminal.cols));
  const y = terminal.buffer.active.viewportY + Math.floor((clientY - rect.top) / (rect.height / terminal.rows));
  const urls = findTerminalUrls(at => at < 0 ? undefined : bufferRow(terminal, at), y, terminal.cols);
  return terminalUrlAtCell(urls, x + 1, y + 1);
}

/** Links from escape codes get xterm's own provider, which only knows one row at a time and
 * comes first. There's no public way to rank above it, so a URL would underline row by row.
 * Should this ever stop working those links stay clickable, one row underlined at a time. */
function rankFirst(terminal: Terminal, provider: ILinkProvider) {
  const internal = terminal as unknown as { _core?: { _linkProviderService?: { linkProviders?: ILinkProvider[] } } };
  const providers = internal._core?._linkProviderService?.linkProviders;
  const index = providers?.indexOf(provider) ?? -1;
  if (index > 0) { providers!.splice(index, 1); providers!.unshift(provider); }
}

/** URLs in a terminal open on this machine, wherever the shell runs. */
export function installTerminalLinks(terminal: Terminal, open: (url: string) => void): IDisposable {
  // Ending a drag inside a URL selects its text, it doesn't follow it.
  const follow = (url: string) => { if (!terminal.hasSelection()) open(url); };
  terminal.options.linkHandler = { activate: (_event, url) => follow(url) };
  const provider: ILinkProvider = {
    provideLinks(y, callback) {
      const urls = findTerminalUrls(at => at < 0 ? undefined : bufferRow(terminal, at), y - 1, terminal.cols);
      callback(urls.length ? urls.map(({ url, start, end }) => ({
        text: url, range: { start, end }, activate: (_event, target) => follow(target),
      })) : undefined);
    },
  };
  const registration = terminal.registerLinkProvider(provider);
  rankFirst(terminal, provider);
  return registration;
}

export function openTerminalLink(url: string): Promise<void> {
  if (isTauri()) return invoke("open_website", { url });
  window.open(url, "_blank", "noopener");
  return Promise.resolve();
}
