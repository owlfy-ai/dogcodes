import { describe, expect, it } from "vitest";
import type { ProjectId, ProjectSummary } from "@dogcodes/protocol";
import { projectOptions } from "./ProjectPicker";

const project = (id: string, displayName: string, path: string): ProjectSummary => ({additionalDirectories: [], 
  projectId: id as ProjectId, displayName, path, taskCount: 0,
});
const projects = [
  project("scratch", "快速对话", "/tmp/scratch"),
  project("one", "dogcodes", "/workspace/code"),
  project("alias", "Alias", "/workspace/code"),
  project("two", "Website", "/workspace/site"),
];
describe("projectOptions", () => {
  it("keeps distinct projects with the same primary directory and excludes scratch", () => {
    expect(projectOptions(projects, "").map(item => item.projectId)).toEqual(["one", "alias", "two"]);
  });
  it("searches both names and paths ignoring case and surrounding whitespace", () => {
    expect(projectOptions(projects, " DOG ")[0].projectId).toBe("one");
    expect(projectOptions(projects, "/WORKSPACE/SITE")[0].projectId).toBe("two");
    expect(projectOptions(projects, "missing")).toEqual([]);
  });
});

it("searches home-relative paths as well as full primary and additional paths", () => {
  const items = [{ ...project("home", "App", "/home/alice/code/app"), additionalDirectories: ["/home/alice/shared"] }];
  expect(projectOptions(items, "~/code", "/home/alice")).toEqual(items);
  expect(projectOptions(items, "~/shared", "/home/alice")).toEqual(items);
  expect(projectOptions(items, "/home/alice/code", "/home/alice")).toEqual(items);
});
