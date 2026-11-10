import type { TurnArtifact } from "./turnArtifactCache";

type CapturedTurn = TurnArtifact["data"];

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

const omitted = (version: unknown) =>
  typeof version === "object" && version !== null && (version as { contentOmitted?: unknown }).contentOmitted === true;

/** Whether the whole turn can be restored. Mirrors the checks `undo_turn`
 * (crates/workspace/src/review/recovery/turn.rs) makes before touching files,
 * so the button is not offered for a capture it would refuse. */
export function turnUndoAvailable(data: CapturedTurn): boolean {
  return data.attribution === "workspace_interval"
    && data.recoveredAfterInterruption !== true
    && !turnSharedWithOthers(data)
    && data.files.length > 0
    && data.files.every(file => !omitted(file.before) && !omitted(file.after));
}
