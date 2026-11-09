import type { ProjectId, TaskId, TaskSummary } from "@dogcodes/protocol";

/** The project the core registers for a worktree fork shares the fork's ULID (core task_fork.rs). */
export const forkWorkspaceProjectId = (taskId: TaskId) => taskId.replace(/^tsk_/, "prj_") as ProjectId;

/**
 * Nests forks under the conversation they came from. `sections` names where each
 * listed conversation appears; a fork nests only in the same section as its
 * source and otherwise stays a top-level row of its own.
 */
export function forkTree(tasks: TaskSummary[], sections: ReadonlyMap<TaskId, string>) {
  const children = new Map<TaskId, TaskSummary[]>();
  const nested = new Set<TaskId>();
  // `tasks` is in creation order, so each source lists its forks oldest first.
  const sourceOf = new Map(tasks.map(task => [task.taskId, task.forkedFrom ?? null]));
  // A corrupt cycle of sources would hide every row in it; keep such rows top-level.
  const cyclic = (id: TaskId) => {
    const seen = new Set([id]);
    for (let next = sourceOf.get(id); next; next = sourceOf.get(next)) { if (seen.has(next)) return true; seen.add(next); }
    return false;
  };
  for (const task of tasks) {
    const source = task.forkedFrom;
    const section = sections.get(task.taskId);
    if (!source || !section || sections.get(source) !== section || cyclic(task.taskId)) continue;
    nested.add(task.taskId);
    children.set(source, [...(children.get(source) ?? []), task]);
  }
  const descendants = (id: TaskId, seen = new Set<TaskId>()): TaskSummary[] =>
    (children.get(id) ?? []).flatMap(child => seen.has(child.taskId) ? [] : (seen.add(child.taskId), [child, ...descendants(child.taskId, seen)]));
  return { children, nested, descendants };
}

/** Worktree-fork projects exist only to hold a fork's files; the fork is listed under its source instead. */
export function forkWorkspaceProjects(tasks: TaskSummary[]) {
  return new Set(tasks.filter(task => task.forkedFrom).map(task => forkWorkspaceProjectId(task.taskId)));
}
