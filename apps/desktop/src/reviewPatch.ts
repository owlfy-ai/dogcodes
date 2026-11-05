import { artifactVisibleText } from "./artifactView";
import type { DiffHunk } from "./diffView";
import type { TextComparison } from "./reviewClient";

/** Convert only captured lines; hunk offsets stay separate so gaps never become invented content. */
export function reviewPatchHunk(hunk: DiffHunk): TextComparison {
  const diff: TextComparison = { before: [], after: [], blocks: [], additions: 0, deletions: 0 };
  let previousKind: "context" | "addition" | "deletion" | undefined;
  for (const line of hunk.lines) {
    if (line.kind === "no_newline") {
      if (previousKind !== "addition" && diff.before.length) diff.before[diff.before.length - 1] = diff.before.at(-1)!.slice(0, -1);
      if (previousKind !== "deletion" && diff.after.length) diff.after[diff.after.length - 1] = diff.after.at(-1)!.slice(0, -1);
      continue;
    }
    previousKind = line.kind;
    const oldStart = diff.before.length, newStart = diff.after.length;
    const text = `${artifactVisibleText(line.text)}\n`;
    if (line.kind !== "addition") diff.before.push(text);
    if (line.kind !== "deletion") diff.after.push(text);
    if (line.kind === "addition") diff.additions++;
    if (line.kind === "deletion") diff.deletions++;
    const kind = line.kind === "context" ? "equal" : "replace";
    const previous = diff.blocks.at(-1);
    if (previous?.kind === kind) {
      previous.oldEnd = diff.before.length; previous.newEnd = diff.after.length;
    } else {
      diff.blocks.push({ kind, oldStart, newStart, oldEnd: diff.before.length, newEnd: diff.after.length });
    }
  }
  return diff;
}
