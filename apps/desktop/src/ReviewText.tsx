import { highlightedLines, languageForPath } from "./syntaxHighlight";
import { useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { TextComparison } from "./reviewClient";
import { ReviewReadAnchor } from "./ReviewReadAnchor";
import "./review.css";

// The parent keys this component by file + comparison options, not by refresh ID.
export function ReviewText({ diff, split, showStatistics = true, path, focusLine }: { diff: TextComparison; split: boolean; showStatistics?: boolean; path?: string; focusLine?: {side: "before" | "after"; start: number; end: number} }) {
  // Anchor expansion to the original file's line ranges. New edits may split
  // an equal block or shift its working-side coordinates without losing the
  // context the user has already opened.
  const element = useRef<HTMLElement>(null);
  useEffect(() => {
    element.current?.querySelector<HTMLElement>('[data-review-focused="true"]')?.scrollIntoView?.({ block: "center" });
  }, [diff, focusLine?.side, focusLine?.start, focusLine?.end]);
  const focused = (oldIndex?: number, newIndex?: number) => {
    const index = focusLine?.side === "before" ? oldIndex : newIndex;
    return !!focusLine && index !== undefined && index + 1 >= focusLine.start && index + 1 <= focusLine.end;
  };
  const [expanded, setExpanded] = useState<Array<[number, number]>>([]);
  const lineNumberDigits = Math.max(2, String(Math.max(diff.before.length, diff.after.length)).length);
  const language = languageForPath(path);
  const highlighted = useMemo(() => ({
    before: highlightedLines(diff.before.join("").replaceAll("\r\n", "\n"), language),
    after: highlightedLines(diff.after.join("").replaceAll("\r\n", "\n"), language),
  }), [diff.before, diff.after, language]);
  const line = (text: string | undefined, tokens: ReactNode[] | undefined) => text === undefined ? null : <>
    <code>{tokens ?? text.replace(/\r?\n$/, "")}</code>
    {!text.endsWith("\n") && <small className="review-no-newline">文件末尾无换行</small>}
  </>;
  const rows = diff.blocks.flatMap((block, index) => {
    const focusStart = focusLine?.side === "before" ? block.oldStart : block.newStart;
    const focusEnd = focusLine?.side === "before" ? block.oldEnd : block.newEnd;
    const isExpanded = (!!focusLine && focusLine.start <= focusEnd && focusLine.end > focusStart) || expanded.some(([start, end]) => start < block.oldEnd && end > block.oldStart);
    const oldLength = block.oldEnd - block.oldStart;
    const newLength = block.newEnd - block.newStart;
    const equal = block.kind === "equal";
    const count = Math.max(oldLength, newLength);
    const result = [];
    for (let offset = 0; offset < count; offset++) {
      if (equal && count > 8 && offset === 3 && !isExpanded) {
        result.push(<div key={`${index}:fold`} className="review-context"><button type="button" onClick={() => setExpanded(previous => [...previous, [block.oldStart, block.oldEnd]])}>展开 {count - 6} 行未修改内容</button></div>);
        offset = count - 4;
        continue;
      }
      const oldIndex = block.oldStart + offset;
      const newIndex = block.newStart + offset;
      const oldText = offset < oldLength ? diff.before[oldIndex] : undefined;
      const newText = offset < newLength ? diff.after[newIndex] : undefined;
      if (split) {
        result.push(<div className="review-split-row" data-review-focused={focused(oldText !== undefined ? oldIndex : undefined, newText !== undefined ? newIndex : undefined)} data-review-old-line={oldText !== undefined ? oldIndex : undefined} data-review-new-line={newText !== undefined ? newIndex : undefined} key={`${index}:${offset}`}>
          <div className={!equal && oldText !== undefined ? "review-removed" : ""}><span>{oldText !== undefined ? oldIndex + 1 : ""}</span>{line(oldText, highlighted.before[oldIndex])}</div>
          <div className={!equal && newText !== undefined ? "review-added" : ""}><span>{newText !== undefined ? newIndex + 1 : ""}</span>{line(newText, highlighted.after[newIndex])}</div>
        </div>);
      } else if (equal) {
        result.push(<div className="review-inline-row" data-review-focused={focused(oldIndex, newIndex)} data-review-old-line={oldIndex} data-review-new-line={newIndex} key={`${index}:${offset}`}><span>{oldIndex + 1}</span><span>{newIndex + 1}</span><i> </i>{line(newText, highlighted.after[newIndex])}</div>);
      }
    }
    if (!split && !equal) {
      // Keep complete deletion/addition groups together, not alternating rows.
      for (let oldIndex = block.oldStart; oldIndex < block.oldEnd; oldIndex++) result.push(<div className="review-inline-row review-removed" data-review-focused={focused(oldIndex, undefined)} data-review-old-line={oldIndex} key={`${index}:old:${oldIndex}`}><span>{oldIndex + 1}</span><span /><i>−</i>{line(diff.before[oldIndex], highlighted.before[oldIndex])}</div>);
      for (let newIndex = block.newStart; newIndex < block.newEnd; newIndex++) result.push(<div className="review-inline-row review-added" data-review-focused={focused(undefined, newIndex)} data-review-new-line={newIndex} key={`${index}:new:${newIndex}`}><span /><span>{newIndex + 1}</span><i>+</i>{line(diff.after[newIndex], highlighted.after[newIndex])}</div>);
    }
    return result;
  });
  return <ReviewReadAnchor diff={diff}><section ref={element} className="review-text" aria-label={split ? "左右对照差异" : "行内差异"}>
    {showStatistics && <header><span className="review-add-count">+{diff.additions}</span> <span className="review-delete-count">−{diff.deletions}</span></header>}
    {diff.additions === 0 && diff.deletions === 0 && <p>没有文本差异；文件仍可能存在属性变化。</p>}
    <div className="review-code" style={{ "--review-line-width": `calc(${lineNumberDigits}ch + 8px)` } as CSSProperties}>{rows}</div>
  </section></ReviewReadAnchor>;
}
