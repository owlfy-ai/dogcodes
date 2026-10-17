import { describe, expect, it } from "vitest";
import { reviewGitFailure } from "./reviewGitError";

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
