import { describe, expect, it } from "vitest";
import { changeSelection, RequestEpoch } from "./requestEpoch";

describe("conversation request ordering", () => {
  it("does not clear a restored conversation when clicking its active row", () => {
    let timeline = ["restored reply"];
    changeSelection("A", "A", () => { timeline = []; });
    expect(timeline).toEqual(["restored reply"]);
    changeSelection("A", "B", () => { timeline = []; });
    expect(timeline).toEqual([]);
  });

  it("rejects an old empty startup read after a newer event refresh", async () => {
    const epoch = new RequestEpoch();
    let finish!: (items: string[]) => void;
    let displayed: string[] = [];
    const old = epoch.begin();
    const pending = new Promise<string[]>(resolve => { finish = resolve; })
      .then(items => { if (old()) displayed = items; });
    const fresh = epoch.begin();
    if (fresh()) displayed = ["reply"];
    finish([]);
    await pending;
    expect(displayed).toEqual(["reply"]);
  });

  it("invalidates old approvals and messages even on A -> B -> A", () => {
    const epoch = new RequestEpoch();
    const firstA = epoch.begin();
    epoch.invalidate();
    const b = epoch.begin();
    epoch.invalidate();
    const secondA = epoch.begin();
    expect(firstA()).toBe(false);
    expect(b()).toBe(false);
    expect(secondA()).toBe(true);
    epoch.invalidate();
    expect(secondA()).toBe(false);
  });
});
