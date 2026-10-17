import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";

const parser = unified().use(remarkParse).use(remarkGfm).use(remarkMath);
type Node = { type: string; children?: Node[] };
const hasDefinition = (node: Node): boolean =>
  node.type === "definition" || node.type === "footnoteDefinition" || !!node.children?.some(hasDefinition);

export type MarkdownBlock = { offset: number; text: string; pending: boolean };

/** Keep the last two top-level nodes reparsable: an incomplete new list marker
 * can still merge with the previous list across blank lines. Definitions require document-wide
 * resolution, so those documents deliberately use the canonical full parser. */
export class StreamingMarkdown {
  private previous = "";
  private stable: MarkdownBlock[] = [];
  private boundary = 0;
  private wholeDocument = false;

  update(text: string, streaming: boolean): MarkdownBlock[] {
    if (!text.startsWith(this.previous)) {
      this.stable = []; this.boundary = 0; this.wholeDocument = false;
    }
    this.previous = text;
    if (!streaming || this.wholeDocument) return [{ offset: 0, text, pending: streaming }];
    const tail = text.slice(this.boundary);
    const tree = parser.parse(tail);
    if (hasDefinition(tree)) {
      this.wholeDocument = true;
      return [{ offset: 0, text, pending: streaming }];
    }
    const children = tree.children;
    const freezeCount = Math.max(0, children.length - 2);
    const blocks: MarkdownBlock[] = [];
    for (let index = 0; index < children.length; index++) {
      const start = children[index].position?.start.offset ?? 0;
      const end = children[index + 1]?.position?.start.offset ?? tail.length;
      const block = { offset: this.boundary + start, text: tail.slice(start, end), pending: index === children.length - 1 };
      if (index < freezeCount) this.stable.push(block);
      else blocks.push(block);
    }
    if (freezeCount) this.boundary += children[freezeCount].position?.start.offset ?? 0;
    return [...this.stable, ...blocks];
  }
}
