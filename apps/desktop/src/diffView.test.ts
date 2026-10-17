import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { execFileSync } from "node:child_process";
import { chmodSync, mkdtempSync, mkdirSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseGitDiff, type StructuredDiff } from "./diffView";
import { StructuredDiffView } from "./StructuredDiffView";

const bytes = (value: string) => new TextEncoder().encode(value);
function parsed(value: string | Uint8Array): StructuredDiff {
  const result = parseGitDiff(typeof value === "string" ? bytes(value) : value);
  expect(result.kind, result.kind === "fallback" ? result.reason : "").toBe("structured");
  if (result.kind !== "structured") throw new Error(result.reason);
  return result.diff;
}

describe("read-only structured Git diff", () => {
  it("preserves file and hunk order and computes old/new line numbers", () => {
    const diff = parsed("diff --git a/src/a.ts b/src/a.ts\nindex abc..def 100644\n--- a/src/a.ts\n+++ b/src/a.ts\n@@ -2,3 +2,4 @@ function test()\n same\n-old\n+new\n+extra\n tail\n@@ -20 +21 @@\n-before\n+after\n\\ No newline at end of file\n");
    expect(diff.files).toHaveLength(1);
    expect(diff.files[0]!.hunks).toHaveLength(2);
    expect(diff.files[0]!.hunks[0]!.lines.map((line) => [line.kind, line.oldLine, line.newLine])).toEqual([
      ["context", 2, 2], ["deletion", 3, null], ["addition", null, 3], ["addition", null, 4], ["context", 4, 5],
    ]);
    expect(diff.additions).toBe(3);
    expect(diff.deletions).toBe(2);
    expect(diff.files[0]!.hunks[1]!.lines[2]!.kind).toBe("no_newline");
  });

  it("represents added/deleted/renamed/mode-only and binary metadata", () => {
    const diff = parsed("diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+new\n" +
      "diff --git a/old.txt b/old.txt\ndeleted file mode 100644\n--- a/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n" +
      "diff --git a/before.txt b/after.txt\nsimilarity index 100%\nrename from before.txt\nrename to after.txt\n" +
      "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n" +
      "diff --git a/blob.bin b/blob.bin\nindex abc..def 100644\nGIT binary patch\nliteral 2\nAcmePayload\n\n");
    expect(diff.files.map((file) => file.change)).toEqual(["added", "deleted", "renamed", "modified", "binary"]);
    expect(diff.files[2]).toMatchObject({ oldPath: "before.txt", newPath: "after.txt", similarity: "100%" });
    expect(diff.files[3]).toMatchObject({ oldMode: "100644", newMode: "100755", hunks: [] });
    expect(diff.files[4]!.binary).toBe(true);
  });

  it("decodes Git quoted UTF-8 paths without executing or resolving them", () => {
    const diff = parsed('diff --git "a/\\344\\270\\255\\346\\226\\207\\n.txt" "b/\\344\\270\\255\\346\\226\\207\\n.txt"\nold mode 100644\nnew mode 100755\n');
    expect(diff.files[0]!.newPath).toBe("中文\n.txt");
    expect(renderToStaticMarkup(createElement(StructuredDiffView, { diff, artifactKey: "paths" }))).toContain("中文\\n.txt");
    const hostile = parsed('diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -0,0 +1 @@\n+<img src="https://example.invalid/track">\x1b[2J\u202e\n');
    const markup = renderToStaticMarkup(createElement(StructuredDiffView, { diff: hostile, artifactKey: "test" }));
    expect(markup).not.toContain("<img");
    expect(markup).toContain("&lt;img");
    expect(markup).toContain("\\u001b[2J\\u202e");
  });

  it("falls back explicitly for non-UTF-8, unsupported or incomplete hunks", () => {
    expect(parseGitDiff(new Uint8Array([255])).kind).toBe("fallback");
    expect(parseGitDiff(bytes("not a patch\n")).kind).toBe("fallback");
    expect(parseGitDiff(bytes("diff --cc combined\n")).kind).toBe("fallback");
    expect(parseGitDiff(bytes("diff --git a/a b/a\n@@ -1,2 +1,2 @@\n one\n")).kind).toBe("fallback");
    expect(parseGitDiff(bytes("diff --git a/a b/a\n@@ -1 +1 @@\n one")).kind).toBe("fallback");
    expect(parseGitDiff(bytes("diff --git a/a b/a\n")).kind).toBe("fallback");
    expect(parseGitDiff(bytes("diff --git a/a b/a\nold mode wrong\nnew mode 100755\n")).kind).toBe("fallback");
    expect(parseGitDiff(new Uint8Array(1_048_577)).kind).toBe("fallback");
    expect(parsed("").files).toEqual([]);
  });

  it("bounds structured rendering and reports a partial view without mutating source bytes", () => {
    const source = bytes(`diff --git a/a b/a\nnew file mode 100644\n--- /dev/null\n+++ b/a\n@@ -0,0 +1,10001 @@\n${"+x\n".repeat(10001)}`);
    const original = source.slice();
    const diff = parsed(source);
    expect(diff.truncated).toBe(true);
    expect(diff.truncationReason).toContain("10000");
    expect(diff.files[0]!.hunks[0]!.lines).toHaveLength(10000);
    expect(source).toEqual(original);
  });

  it("bounds file and hunk counts independently of byte size", () => {
    const files = parsed(Array.from({ length: 501 }, (_, index) => `diff --git a/f${index} b/f${index}\nold mode 100644\nnew mode 100755\n`).join(""));
    expect(files.files).toHaveLength(500);
    expect(files.truncationReason).toContain("500");
    const hunks = parsed("diff --git a/a b/a\n--- a/a\n+++ b/a\n" + Array.from({ length: 2001 }, (_, index) => `@@ -${index + 1} +${index + 1} @@\n x\n`).join(""));
    expect(hunks.files[0]!.hunks).toHaveLength(2000);
    expect(hunks.truncationReason).toContain("2000");
  });

  it("rejects incomplete metadata and invalid line ranges instead of inventing locations", () => {
    for (const patch of [
      "diff --git a/a b/a\nindex abc..def 100644\n",
      "diff --git a/a b/b\nrename from a\n",
      "diff --git a/a b/a\n@@ -0 +0 @@\n x\n",
      "diff --git a/a b/a\n@@ -9007199254740991 +1 @@\n x\n",
    ]) expect(parseGitDiff(bytes(patch)).kind).toBe("fallback");
  });

  it("parses real Git output for spaces, Unicode, rename, deletion and binary files", () => {
    const fixtureRoot = mkdtempSync(join(tmpdir(), "dogcodes-diff-view-"));
    const directory = join(fixtureRoot, "repo");
    const configPath = join(fixtureRoot, "empty-git-config");
    mkdirSync(directory); writeFileSync(configPath, "");
    const unusualPaths = process.platform === "win32" ? [] : ["line\nbreak.txt", "tab\tname.txt", "trailing space ", 'quote"name.txt'];
    const git = (...args: string[]) => execFileSync("git", args, { cwd: directory,
      env: { PATH: process.env.PATH, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: configPath, LC_ALL: "C" }, timeout: 5000 });
    try {
      git("init", "--quiet");
      git("config", "user.name", "Diff Test"); git("config", "user.email", "diff@example.invalid");
      mkdirSync(join(directory, "dir space"));
      writeFileSync(join(directory, "dir space", "hello world.txt"), "before\n");
      writeFileSync(join(directory, "中文.txt"), "unicode before\n");
      writeFileSync(join(directory, "rename.txt"), "rename content\n");
      writeFileSync(join(directory, "delete.txt"), "delete\n");
      writeFileSync(join(directory, "blob.bin"), new Uint8Array([0, 1, 2]));
      writeFileSync(join(directory, "run.sh"), "#!/bin/sh\n");
      for (const name of unusualPaths) writeFileSync(join(directory, name), "before\n");
      if (process.platform !== "win32") symlinkSync("rename.txt", join(directory, "link"));
      git("add", "-A"); git("commit", "--quiet", "-m", "baseline");
      writeFileSync(join(directory, "dir space", "hello world.txt"), "after\n");
      writeFileSync(join(directory, "中文.txt"), "unicode after\n");
      renameSync(join(directory, "rename.txt"), join(directory, "renamed.txt"));
      rmSync(join(directory, "delete.txt"));
      writeFileSync(join(directory, "blob.bin"), new Uint8Array([0, 3, 4]));
      for (const name of unusualPaths) writeFileSync(join(directory, name), "after\n");
      if (process.platform !== "win32") {
        chmodSync(join(directory, "run.sh"), 0o755);
        rmSync(join(directory, "link")); symlinkSync("renamed.txt", join(directory, "link"));
      }
      git("add", "-A");
      const diff = parsed(new Uint8Array(git("-c", "core.quotepath=true", "diff", "--cached", "--binary", "--full-index", "--no-ext-diff", "--no-textconv", "HEAD", "--", ".")));
      expect(diff.files.map((file) => file.newPath)).toContain("dir space/hello world.txt");
      expect(diff.files.map((file) => file.newPath)).toContain("中文.txt");
      expect(diff.files.find((file) => file.change === "renamed")?.newPath).toBe("renamed.txt");
      expect(diff.files.some((file) => file.change === "deleted")).toBe(true);
      expect(diff.files.some((file) => file.binary)).toBe(true);
      for (const name of unusualPaths) expect(diff.files.map((file) => file.newPath)).toContain(name);
      if (process.platform !== "win32") {
        expect(diff.files.find((file) => file.newPath === "run.sh")?.newMode).toBe("100755");
        expect(diff.files.find((file) => file.newPath === "link")?.newMode).toBe("120000");
      }
    } finally { rmSync(fixtureRoot, { recursive: true, force: true }); }
  });
});
