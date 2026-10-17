import { expect, it } from "vitest";
import { reviewFingerprint } from "./reviewFingerprint";
import type { ReviewSnapshot } from "./reviewClient";

it("ignores snapshot IDs and statistics but detects content and scope changes", () => {
  const snapshot: ReviewSnapshot = { id: "one", taskId: "task", roots: [{ root: { id: "root", path: "/root", git: true, scopes: [""] }, mode: "uncommitted", base: "head", target: "working", files: [], untracked: [], error: null, statistics: {} }] };
  const copy = structuredClone(snapshot);
  copy.id = "two";
  copy.roots[0].statistics = { x: { additions: 2, deletions: 1, binary: false, error: null } };
  expect(reviewFingerprint(copy)).toBe(reviewFingerprint(snapshot));
  copy.roots[0].root.scopes = ["src"];
  expect(reviewFingerprint(copy)).not.toBe(reviewFingerprint(snapshot));
  copy.roots[0].root.scopes = [""];
  copy.roots[0].files = [{ path: "file", kind: "added", before: null, after: { hash: "sha", bytes: 1, kind: "file", executable: false, binary: false } }];
  expect(reviewFingerprint(copy)).not.toBe(reviewFingerprint(snapshot));
});
