import { invoke } from "@tauri-apps/api/core";
import { reviewMutations } from "./reviewMutations";

export type ReviewMode = "uncommitted" | "unstaged" | "staged" | "committed" | "branch" | "session";
export interface ReviewRoot { id: string; path: string; git: boolean; scopes: string[] }
export interface FileVersion { hash: string; bytes: number; kind: "file" | "symlink" | "gitlink"; executable: boolean; binary: boolean }
export interface ReviewChange { path: string; previousPath?: string | null; kind: "added" | "deleted" | "modified" | "renamed"; before: FileVersion | null; after: FileVersion | null }
export interface FileStatistics { additions: number | null; deletions: number | null; binary: boolean; error: string | null }
export interface RootChanges { root: ReviewRoot; mode: ReviewMode; base: string; target: string; files: ReviewChange[]; untracked: string[]; excludedRoots?: string[]; error: string | null; statistics: Record<string, FileStatistics> }
export interface ReviewSnapshot { id: string; taskId: string; roots: RootChanges[] }
export interface ReviewLeaseRequest { taskId: string; snapshotId: string; leaseId: string; revision: number; release: boolean }
export interface ReviewQuery { taskId: string; rootId?: string; mode: ReviewMode; reference?: string; parent?: number; probe?: boolean }
export interface TextBlock { kind: "equal" | "insert" | "delete" | "replace"; oldStart: number; oldEnd: number; newStart: number; newEnd: number }
export interface TextComparison { before: string[]; after: string[]; blocks: TextBlock[]; additions: number; deletions: number }
export interface ReviewFile { binary: boolean; text: TextComparison | null; beforeImage?: string | null; afterImage?: string | null }
export interface ReviewCommit { id: string; parents: string[]; author: string; date: string; subject: string }
export interface ReviewBranch { name: string; commit: string }
export interface FilePreview { text: string | null; image: string | null }
export interface FileBatchResult { completed: string[]; error: string | null }
export interface BatchFileRequest { taskId: string; snapshotId: string; rootId: string; paths: string[]; action: "discard" | "ignore"; confirmed?: boolean }
export interface DiscardRequest { taskId: string; snapshotId: string; rootId: string; path: string; confirmed: boolean }
// Share only in-flight reads. Completed snapshots are owned/leased by the reader;
// explicit refreshes must always query current disk state.
const pendingQueries = new Map<string, Promise<ReviewSnapshot>>();
function querySnapshot(query: ReviewQuery): Promise<ReviewSnapshot> {
  const key = JSON.stringify([query.taskId, query.rootId ?? "", query.mode, query.reference ?? "", query.parent ?? 0, query.probe ?? false]);
  const pending = pendingQueries.get(key);
  if (pending) return pending;
  const request = invoke<ReviewSnapshot>("core_review_query", { query }).finally(() => pendingQueries.delete(key));
  pendingQueries.set(key, request);
  return request;
}

export const reviewClient = {
  batchFiles: (request: BatchFileRequest) => reviewMutations.run<FileBatchResult>("core_review_batch_files", { request: { ...request, confirmed: request.confirmed ?? false } }, request.taskId, request.rootId),
  lease: (request: ReviewLeaseRequest) => invoke<void>("core_review_snapshot_lease", { request }),
  discard: (request: DiscardRequest) => reviewMutations.run<string>("core_review_discard", { request }, request.taskId, request.rootId),
  preview: (taskId: string, snapshotId: string, rootId: string, path: string, version: "working" | "head") => invoke<FilePreview>("core_review_preview", { taskId, snapshotId, rootId, path, version }),
  reveal: (taskId: string, snapshotId: string, rootId: string, path: string) => invoke<void>("core_review_reveal", { taskId, snapshotId, rootId, path }),
  ignore: (taskId: string, snapshotId: string, rootId: string, path: string) => reviewMutations.run<void>("core_review_ignore", { taskId, snapshotId, rootId, path }, taskId, rootId),
  index: (taskId: string, snapshotId: string, rootId: string, path: string | string[], action: "stage" | "unstage") =>
    reviewMutations.run<void>("core_review_index", { taskId, snapshotId, rootId, paths: typeof path === "string" ? [path] : path, action }, taskId, rootId),
  query: querySnapshot,
  file: (taskId: string, snapshotId: string, rootId: string, path: string) =>
    invoke<ReviewFile>("core_review_file", { taskId, snapshotId, rootId, path }),
  history: (taskId: string, rootId: string, skip = 0) => invoke<ReviewCommit[]>("core_review_history", { taskId, rootId, skip }),
  branches: (taskId: string, rootId: string) => invoke<ReviewBranch[]>("core_review_branches", { taskId, rootId }),
};
