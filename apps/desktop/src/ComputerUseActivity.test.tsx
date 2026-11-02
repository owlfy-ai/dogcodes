// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import type { TimelineItem } from "@dogcodes/protocol";
import { ComputerUseActivity } from "./ComputerUseActivity";

it("shows persisted computer actions and errors without exposing raw screenshot or input payloads", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const container = document.createElement("div");
  const root = createRoot(container);
  const item = { kind: "computer_use", text: JSON.stringify({tool:"click", mode:"foreground", elapsedMs:1250, imageCount:2, transportOk:true, isError:false}) } as TimelineItem;
  try {
    await act(async () => root.render(<ComputerUseActivity item={item} />));
    expect(container.querySelector("details")).not.toBeNull();
    expect(container.textContent).toContain("1.25");
    expect(container.textContent).toContain("2");
    expect(container.textContent).not.toContain("elapsedMs");
    const success = container.querySelector("summary small")!.textContent;
    await act(async () => root.render(<ComputerUseActivity item={{...item,text:JSON.stringify({...JSON.parse(item.text),transportOk:false})}} />));
    expect(container.querySelector("summary small")!.textContent).not.toBe(success);
    await act(async () => root.render(<ComputerUseActivity item={{...item,text:"broken"}} />));
    expect(container.textContent).toBe("");
  } finally { await act(async () => root.unmount()); }
});
