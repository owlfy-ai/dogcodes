import { describe, expect, it } from "vitest";
import type { ProjectId, ProjectSummary, TaskId, TaskSummary } from "@dogcodes/protocol";
import { groupTasksByWorkspace } from "./workspaceGroups";
import { timelineItemRole } from "./timelinePresentation";

const project = (id: string, displayName: string, path: string): ProjectSummary => ({additionalDirectories: [], 
  projectId: id as ProjectId,
  displayName,
  path,
  taskCount: 0,
});

const task = (id: string, projectId: string, providerId: string): TaskSummary => ({
  taskId: id as TaskId,
  projectId: projectId as ProjectId,
  title: `会话 ${id}`,
  preferredExecutor: { providerId, installationId: null },
  activePrimaryRun: null,
  streamVersion: 1,
});

describe("groupTasksByWorkspace", () => {
  it("keeps project identities separate even when their primary directory matches", () => {
    const groups = groupTasksByWorkspace(
      [
        project("prj_a", "仓库 A", "/workspace/shared"),
        project("prj_b", "仓库 A 的别名", "/workspace/shared"),
        project("prj_c", "仓库 C", "/workspace/other"),
      ],
      [
        task("tsk_a", "prj_a", "codex"),
        task("tsk_b", "prj_b", "claude-code"),
        task("tsk_c", "prj_c", "kimi"),
      ],
    );

    expect(groups).toHaveLength(3);
    expect(groups[0].path).toBe("/workspace/shared");
    expect(groups[0].tasks.map((item) => item.preferredExecutor.providerId)).toEqual([
      "codex",
    ]);
    expect(groups[1].tasks[0].preferredExecutor.providerId).toBe("claude-code");
    expect(groups[2].tasks[0].preferredExecutor.providerId).toBe("kimi");
  });
});

describe("timelineItemRole", () => {
  it("renders tool lifecycle rows separately from assistant messages", () => {
    expect(timelineItemRole("user_message")).toBe("user");
    expect(timelineItemRole("assistant_message")).toBe("assistant");
    expect(timelineItemRole("tool.started")).toBe("tool");
    expect(timelineItemRole("tool.completed")).toBe("tool");
    expect(timelineItemRole("tool.failed")).toBe("tool");
    expect(timelineItemRole("handoff")).toBe("system");
  });
});
