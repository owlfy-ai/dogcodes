import { diffArrays } from "diff";
import type { ReviewSource } from "@dogcodes/protocol";
import type { TextComparison } from "./reviewClient";
export function sourceDiff(source: ReviewSource): TextComparison | null {
  if (source.before === null || source.after === null) return null;
  const lines = (text: string) => text.match(/[^\n]*\n|[^\n]+$/g) ?? [];
  const before = lines(source.before), after = lines(source.after);
  if (before.length + after.length > 20000) return null;
  const changes = diffArrays(before, after, { timeout: 100, maxEditLength: 5000 });
  if (!changes) return null;
  const diff: TextComparison = {before, after, blocks: [], additions: 0, deletions: 0};
  let oldStart = 0, newStart = 0;
  for (const change of changes) {
    const oldEnd = oldStart + (change.added ? 0 : change.value.length);
    const newEnd = newStart + (change.removed ? 0 : change.value.length);
    diff.blocks.push({kind: change.added ? "insert" : change.removed ? "delete" : "equal", oldStart, oldEnd, newStart, newEnd});
    if (change.added) diff.additions += change.value.length;
    if (change.removed) diff.deletions += change.value.length;
    oldStart = oldEnd; newStart = newEnd;
  }
  return diff;
}
