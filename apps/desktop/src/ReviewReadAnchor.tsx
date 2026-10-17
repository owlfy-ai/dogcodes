import { Component, createRef, type ReactNode } from "react";
import type { TextComparison } from "./reviewClient";

type Props = { diff: TextComparison; children: ReactNode };
type Anchor = { line: string; side: "old" | "new"; top: number; scroller: HTMLElement };

// Map an unchanged working-side line without a quadratic diff. Prefix/suffix
// preserve repeated lines in stable regions; otherwise only unique matches are
// accepted. Ambiguous or removed content keeps the existing pixel position.
function workingLine(previous: string[], current: string[], index: number): number | null {
  let prefix = 0;
  while (prefix < previous.length && prefix < current.length && previous[prefix] === current[prefix]) prefix++;
  if (index < prefix) return index;
  let suffix = 0;
  while (suffix < previous.length - prefix && suffix < current.length - prefix
    && previous[previous.length - 1 - suffix] === current[current.length - 1 - suffix]) suffix++;
  if (index >= previous.length - suffix) return current.length - (previous.length - index);
  const text = previous[index];
  if (text === undefined || previous.indexOf(text) !== previous.lastIndexOf(text)) return null;
  const match = current.indexOf(text);
  return match >= 0 && match === current.lastIndexOf(text) ? match : null;
}

// React's pre-mutation snapshot reads the OLD DOM, even when new diff blocks
// reuse row keys. A layout-effect cleanup is too late for that guarantee.
export class ReviewReadAnchor extends Component<Props, object, Anchor | null> {
  private element = createRef<HTMLDivElement>();

  getSnapshotBeforeUpdate(previous: Props): Anchor | null {
    if (previous.diff === this.props.diff
      || previous.diff.before.length !== this.props.diff.before.length
      || previous.diff.before.some((line, index) => line !== this.props.diff.before[index])) return null;
    const scroller = this.element.current?.closest<HTMLElement>(".review-file-body");
    if (!scroller || scroller.scrollTop === 0) return null;
    const bounds = scroller.getBoundingClientRect();
    const visible = (node: HTMLElement) => {
        const rect = node.getBoundingClientRect();
        return rect.bottom > bounds.top && rect.top < bounds.bottom;
    };
    const row = [...this.element.current!.querySelectorAll<HTMLElement>("[data-review-old-line]")].find(visible);
    if (row) return { line: row.dataset.reviewOldLine!, side: "old", top: row.getBoundingClientRect().top - bounds.top, scroller };
    const added = [...this.element.current!.querySelectorAll<HTMLElement>("[data-review-new-line]")].find(visible);
    if (!added) return null;
    const mapped = workingLine(previous.diff.after, this.props.diff.after, Number(added.dataset.reviewNewLine));
    return mapped === null ? null : { line: String(mapped), side: "new", top: added.getBoundingClientRect().top - bounds.top, scroller };
  }

  componentDidUpdate(_previous: Props, _state: object, anchor: Anchor | null) {
    if (!anchor) return;
    const row = this.element.current?.querySelector<HTMLElement>(`[data-review-${anchor.side}-line="${anchor.line}"]`);
    if (row) anchor.scroller.scrollTop += row.getBoundingClientRect().top
      - anchor.scroller.getBoundingClientRect().top - anchor.top;
  }

  render() {
    return <div ref={this.element} style={{ display: "contents" }}>{this.props.children}</div>;
  }
}
