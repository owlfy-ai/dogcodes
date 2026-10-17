// Structured review replies are consumed by the report UI, not the transcript.
// Recognize unfinished prefixes too, so streamed JSON never flashes on screen.
import { isReviewTransportErrorLine } from "./reviewFailure";
const jsonStart = /^\s*(?:\{(?:\s*$|\s*["}])|\[(?:\s*$|\s*[\[{"\]\d-]|\s*(?:true|false|null)\b))/;
export function reviewActivityText(text: string): string {
  const output: string[] = [];
  let fence: { marker: string; hidden: boolean; untyped: boolean; start: number } | null = null;
  const lines = text.split(/\r?\n/);
  for (const [index, line] of lines.entries()) {
    if (fence) {
      const closing = line.trim();
      if (closing.length >= fence.marker.length && [...closing].every(char => char === fence!.marker[0])) {
        if (!fence.hidden) output.push(line);
        fence = null;
      } else {
        if (fence.untyped && line.trim()) {
          fence.untyped = false;
          if (jsonStart.test(line)) { fence.hidden = true; output.length = fence.start; }
        }
        if (!fence.hidden) output.push(line);
      }
      continue;
    }
    const opening = line.match(/^\s*(`{3,}|~{3,})(.*)$/);
    if (opening) {
      const language = opening[2].trim().toLowerCase();
      const hidden = /^(?:json\b|jsonc\b|application\/json\b)/.test(language) || (index === lines.length - 1 && /^(?:j|js|jso)?$/.test(language));
      fence = { marker: opening[1], hidden, untyped: !language, start: output.length };
      if (!hidden) output.push(line);
      continue;
    }
    // Keep native diagnostics in the failure panel's expandable details.
    if (isReviewTransportErrorLine(line)) continue;
    // A bare object/array is the final transport payload, including partial JSON.
    if (jsonStart.test(line)) break;
    if (index === lines.length - 1 && /^\s*(?:`{1,2}|~{1,2})$/.test(line)) break;
    output.push(line);
  }
  if (fence?.untyped) output.length = fence.start;
  return output.join("\n").trim();
}
