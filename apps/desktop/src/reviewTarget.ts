import type { ReviewSnapshot } from "./reviewClient";

/** Match an artifact path to an actual file in the queried roots, never guess by basename. */
export function reviewTarget(snapshot: ReviewSnapshot, requestedPath: string): { rootId: string; path: string } | null {
  const normalize = (path: string) => path.replaceAll("\\", "/").replace(/\/+$/, "").replace(/^\.\//, "");
  const requested = normalize(requestedPath);
  const matches = snapshot.roots.flatMap(({ root, files }) => files.filter(file =>
    [file.path, file.previousPath].filter((path): path is string => Boolean(path)).some(path =>
      normalize(path) === requested || `${normalize(root.path)}/${normalize(path)}` === requested
        || (requested.startsWith("~/") && `${normalize(root.path)}/${normalize(path)}`.endsWith(requested.slice(1))),
    ),
  ).map(file => ({ rootId: root.id, path: file.path })));
  return matches.length === 1 ? matches[0] : null;
}

/** Preserve root identity for reports containing a repository-relative path. */
export function reviewFindingPath(finding: import("@dogcodes/protocol").ReviewFinding): string {
  const path = (finding.path ?? "").replaceAll("\\", "/");
  if (/^(\/|[A-Za-z]:\/|~\/)/.test(path)) return path;
  const root = [finding.rootLabel, finding.rootId].map(value => value?.replaceAll("\\", "/")).find(value => value && /^(\/|[A-Za-z]:\/|~\/)/.test(value));
  return root ? `${root.replaceAll("\\", "/").replace(/\/$/, "")}/${path}` : path;
}

export function reviewFindingTarget(snapshot: ReviewSnapshot, finding: import("@dogcodes/protocol").ReviewFinding) {
  if (!finding.path) return null;
  const path = reviewFindingPath(finding);
  // Use an explicit known root first, including legacy root IDs; never basename-match files.
  const roots = snapshot.roots.filter(({root}) => [finding.rootId, finding.rootLabel].includes(root.id) || [finding.rootId, finding.rootLabel].includes(root.path));
  return reviewTarget(roots.length ? {...snapshot, roots} : snapshot, path);
}
