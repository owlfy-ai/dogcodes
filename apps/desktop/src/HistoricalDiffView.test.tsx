// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { parseGitDiff, type StructuredDiff } from "./diffView";
import { HistoricalDiffView } from "./HistoricalDiffView";
import { reviewPatchHunk } from "./reviewPatch";

let root: Root, host: HTMLDivElement;
beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  host = document.createElement("div"); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
const parse = (patch: string): StructuredDiff => {
  const result = parseGitDiff(new TextEncoder().encode(patch));
  if (result.kind !== "structured") throw new Error(result.reason);
  return result.diff;
};
const patch = "diff --git a/src/example.ts b/src/example.ts\n--- a/src/example.ts\n+++ b/src/example.ts\n@@ -10,3 +20,4 @@\n keep\n-const value = 1;\n+const value = 2;\n+const extra = true;\n tail\n@@ -900000000 +900000001 @@\n-old\n+new\n\\ No newline at end of file\n" +
  "diff --git a/hello.txt b/hello.txt\nnew file mode 100644\n--- /dev/null\n+++ b/hello.txt\n@@ -0,0 +1 @@\n+hello\n";

it("uses the Git reader and switches layouts while retaining original hunk line numbers", async () => {
  const diff = parse(patch), original = JSON.stringify(diff);
  await act(async () => root.render(<HistoricalDiffView diff={diff} initialPath="src/example.ts" />));
  expect(host.querySelectorAll(".review-inline-row")).toHaveLength(7);
  expect(host.querySelector(".hljs-keyword")?.textContent).toBe("const");
  const removed = [...host.querySelectorAll(".review-inline-row.review-removed")];
  expect(removed[0].children[0].textContent).toBe("11");
  expect(removed[1].children[0].textContent).toBe("900000000");
  expect(host.querySelectorAll(".review-no-newline")).toHaveLength(1);
  const toggle = host.querySelector<HTMLButtonElement>(".review-split-toggle")!;
  await act(async () => toggle.click());
  expect(toggle.getAttribute("aria-pressed")).toBe("true");
  const rows = host.querySelectorAll(".review-split-row");
  expect(rows).toHaveLength(5);
  expect(rows[1].children[0].textContent).toContain("11");
  expect(rows[1].children[1].textContent).toContain("21");
  expect(rows[2].children[0].textContent).toBe("");
  expect(rows[2].children[1].textContent).toContain("22");
  expect(rows[4].children[1].textContent).toContain("900000001");
  expect(JSON.stringify(diff)).toBe(original);
});

it("opens the requested file and resets the reader when choosing another captured file", async () => {
  await act(async () => root.render(<HistoricalDiffView diff={parse(patch)} initialPath="hello.txt" />));
  expect(host.querySelector(".review-file-heading strong")?.textContent).toBe("hello.txt");
  expect(host.querySelector(".review-added code")?.textContent).toBe("hello");
  expect(host.querySelector(".review-added > span:nth-child(2)")?.textContent).toBe("1");
  const body = host.querySelector<HTMLElement>(".review-file-body")!; body.scrollTop = 200;
  await act(async () => host.querySelector<HTMLButtonElement>('button[aria-label="src/example.ts"]')!.click());
  expect(body.scrollTop).toBe(0);
  expect(host.querySelector('button[aria-current="true"]')?.getAttribute("aria-label")).toBe("src/example.ts");
  expect(host.querySelector(".review-files-resize-handle")).not.toBeNull();
});

it("retains binary, rename, mode-only and truncation information", async () => {
  const diff = parse("diff --git a/a.bin b/a.bin\nBinary files a/a.bin and b/a.bin differ\n" +
    "diff --git a/old.ts b/new.ts\nsimilarity index 100%\nrename from old.ts\nrename to new.ts\n" +
    "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n");
  diff.truncated = true; diff.truncationReason = "captured limit";
  await act(async () => root.render(<HistoricalDiffView diff={diff} />));
  expect(host.querySelector(".review-binary-placeholder")).not.toBeNull();
  expect(host.textContent).toContain("captured limit");
  await act(async () => host.querySelector<HTMLButtonElement>('button[aria-label="new.ts"]')!.click());
  expect(host.querySelector(".historical-diff-metadata")?.textContent).toContain("old.ts");
  expect(host.querySelector(".historical-diff-metadata")?.textContent).toContain("100%");
  await act(async () => host.querySelector<HTMLButtonElement>('button[aria-label="run.sh"]')!.click());
  expect(host.querySelector(".historical-diff-metadata")?.textContent).toContain("100644 → 100755");
  expect(host.querySelector(".review-code")).toBeNull();
});

it("preserves EOF markers on each version and escapes untrusted captured text", async () => {
  const diff = parse('diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+<img src="https://example.invalid/track">\u001b[2J\u202e\n\\ No newline at end of file\n');
  const hunk = reviewPatchHunk(diff.files[0].hunks[0]);
  expect(hunk.before).toEqual(["old"]);
  expect(hunk.after[0]).toContain("\\u001b[2J\\u202e");
  expect(hunk.after[0].endsWith("\n")).toBe(false);
  await act(async () => root.render(<HistoricalDiffView diff={diff} />));
  expect(host.querySelector("img")).toBeNull();
  expect(host.querySelectorAll(".review-no-newline")).toHaveLength(2);
});
