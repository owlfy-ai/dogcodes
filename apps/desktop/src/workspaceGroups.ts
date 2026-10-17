import type { ProjectSummary, TaskSummary } from "@dogcodes/protocol";

export function groupTasksByWorkspace(projects: ProjectSummary[], tasks: TaskSummary[]) {
  return projects.map(project => ({
    key: project.projectId,
    path: project.path,
    directories: [project.path, ...project.additionalDirectories],
    displayName: project.displayName,
    tasks: tasks.filter(task => task.projectId === project.projectId),
  }));
}
