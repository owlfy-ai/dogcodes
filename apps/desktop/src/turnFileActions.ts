import type { TurnArtifact } from "./turnArtifactCache";

type CapturedTurn = Omit<TurnArtifact["data"], "files"> & { files: { before?: unknown; after?: unknown }[] };

/** Captured while another conversation ran in the same folder, so the files
 * may include that conversation's writes. */
export const turnSharedWithOthers = (data: CapturedTurn) =>
  data.sharedWorkspace === true || data.attribution === "shared_workspace_interval";

/** Commit only applies to files in a Git repository. A capture of a plain
 * folder (including conversations without a project) records `filesystem`. */
export function turnHasGitRoot(data: CapturedTurn): boolean {
  if (data.roots?.length) return data.roots.some(root => root.changes?.backend === "git");
  // Captures without a recorded backend predate it; keep offering Commit.
  return data.backend === undefined || data.backend === "git";
}
