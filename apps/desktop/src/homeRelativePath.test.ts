import { expect, it } from "vitest";
import { homeRelativePath } from "./homeRelativePath";

it.each([
  ["/Users/alice/Workspace/app", "/Users/alice", "~/Workspace/app"],
  ["/home/alice/code/app", "/home/alice/", "~/code/app"],
  ["/home/alice", "/home/alice", "~"],
  ["/home/alice-old/app", "/home/alice", "/home/alice-old/app"],
  ["/opt/project", "/home/alice", "/opt/project"],
  ["/mnt/home/alice/project", "/home/alice", "/mnt/home/alice/project"],
  ["C:\\Users\\alice\\app", "C:\\Users\\alice", "C:\\Users\\alice\\app"],
  ["/home/alice/app", null, "/home/alice/app"],
])("formats %s relative to %s", (path, home, expected) => {
  expect(homeRelativePath(path, home)).toBe(expected);
});
