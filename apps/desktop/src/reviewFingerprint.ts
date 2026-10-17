import type { ReviewSnapshot } from "./reviewClient";

/** Ignore transport IDs and expensive text statistics, not file versions. */
export function reviewFingerprint(snapshot: ReviewSnapshot): string {
  return JSON.stringify(snapshot.roots.map(root => ({
    root: root.root, mode: root.mode, base: root.base, target: root.target,
    files: root.files, untracked: root.untracked, excludedRoots: root.excludedRoots ?? [], error: root.error,
  })));
}
