import { describe, expect, it } from "vitest";
import { GitPushFailed } from "./gitOperations";
import { GitCommandFailed, reviewGitFailure } from "./reviewGitError";

describe("reviewGitFailure", () => {
  it("keeps Git's own output as collapsible detail", () => {
    const failure = reviewGitFailure("! [rejected] main -> main (fetch first)", "推送失败。", "提交已完成，推送失败：");
    expect(failure.message).toContain("远端有新提交");
    expect(failure.detail).toContain("[rejected]");
  });

  it("never exposes JavaScript exceptions, even as detail", () => {
    const failure = reviewGitFailure(new TypeError("Cannot read properties of undefined (reading 'invoke')"), "拉取失败，请稍后重试。");
    expect(failure).toEqual({ message: "拉取失败，请稍后重试。" });
  });
});

const missingIdentity = "Author identity unknown\n\n*** Please tell me who you are.\n";

it("keeps Git's own output when a command fails", () => {
  const failure = reviewGitFailure(new GitCommandFailed(missingIdentity), "Commit failed");
  expect(failure.message).not.toBe("Commit failed");
  expect(failure.message).toContain("user.name");
  expect(failure.detail).toBe(missingIdentity);
});

it("keeps Git's output for a failed push too", () => {
  expect(reviewGitFailure(new GitPushFailed("rejected"), "Push failed", "PUSH: ").detail).toBe("PUSH: rejected");
});

it("shows only the fallback for an error that is not Git's output", () => {
  expect(reviewGitFailure(new Error("TypeError: x is undefined"), "Commit failed")).toEqual({ message: "Commit failed" });
});
