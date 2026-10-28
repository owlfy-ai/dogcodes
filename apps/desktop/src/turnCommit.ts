import type { RootChanges } from "./reviewClient";

/** A turn's captured files resolved to one Git root of the current review. */
export interface TurnCommitTarget { rootId: string; rootPath: string; paths: string[] }

interface CapturedFile { path: string; workspacePath?: string; relativePath?: string; workspaceRelativePath?: string }
interface CapturedTurn { files: CapturedFile[]; workspacePaths?: string[] }

const slash = (path: string) => path.replace(/\\/g, "/").replace(/\/+$/, "");
const absolute = (path: string) => path.startsWith("/") || /^[a-zA-Z]:\//.test(path);

/** Absolute paths of a turn artifact: v2 records each file's workspace; v1 is
 * relative to its single workspace. */
export function turnAbsolutePaths(data: CapturedTurn): string[] {
  const base = data.workspacePaths?.length === 1 ? slash(data.workspacePaths[0]) : null;
  return data.files.flatMap(file => {
    const path = slash(file.path);
    if (absolute(path)) return [path];
    const workspace = file.workspacePath ? slash(file.workspacePath) : base;
    const relative = slash(file.relativePath ?? file.workspaceRelativePath ?? path);
    return workspace ? [`${workspace}/${relative}`] : [];
  });
}

/** The turn's files that still have uncommitted changes, grouped by Git root.
 * Files committed, undone or edited away since the turn are left out. */
export function turnCommitTargets(data: CapturedTurn, roots: RootChanges[]): TurnCommitTarget[] {
  const files = turnAbsolutePaths(data);
  return roots.flatMap(({ root, files: changes, untracked }) => {
    if (!root.git) return [];
    const rootPath = slash(root.path);
    const changed = new Set([...changes.flatMap(c => [c.path, c.previousPath ?? ""]), ...(untracked ?? [])].filter(Boolean).map(slash));
    const paths = [...new Set(files.filter(file => file.startsWith(`${rootPath}/`)).map(file => file.slice(rootPath.length + 1)))]
      .filter(path => changed.has(path));
    return paths.length ? [{ rootId: root.id, rootPath, paths }] : [];
  });
}
