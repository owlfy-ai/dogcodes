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

it("floats only the newest screenshot of the running turn; rows keep just the description", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const { receivePreview, clearPreviewsForTest } = await import("./computerUsePreviews");
  const { ComputerUsePreviewOverlay } = await import("./ComputerUsePreviewOverlay");
  const container = document.createElement("div");
  const root = createRoot(container);
  const row = (id: string) => ({ itemId: id, kind: "computer_use", text: JSON.stringify({tool:"click", mode:"background", elapsedMs:1060, imageCount:0, transportOk:true}) } as TimelineItem);
  const view = (turnId?: string) => <><ComputerUseActivity item={row("evt_1")} /><ComputerUseActivity item={row("evt_2")} /><ComputerUsePreviewOverlay taskId="tsk" turnId={turnId} /></>;
  const image = "data:image/png;base64,iVBORw0KGgo=";
  try {
    await act(async () => root.render(view("trn_1")));
    expect(container.querySelector("img")).toBeNull();
    await act(async () => receivePreview({ itemId: "evt_1", taskId: "tsk", turnId: "trn_1", image, window: "Finder · test" }));
    await act(async () => receivePreview({ itemId: "evt_2", taskId: "tsk", turnId: "trn_1", image: image + "AA", marker: { x: 0.25, y: 0.8 }, window: "Finder · test", element: "Button \"Recents\"" }));
    // One picture in total: the newest, floating, never inside the rows.
    expect(container.querySelectorAll("img")).toHaveLength(1);
    expect(container.querySelector("details img")).toBeNull();
    expect(container.querySelector(".computer-use-overlay img")!.getAttribute("src")).toBe(image + "AA");
    const marker = container.querySelector<HTMLElement>(".computer-use-marker")!;
    expect(marker.style.left).toBe("25%");
    expect(marker.style.top).toBe("80%");
    expect(container.querySelectorAll("summary")[1].textContent).toContain("Recents");
    // Clicking enlarges it and shows where it is.
    await act(async () => container.querySelector<HTMLButtonElement>(".computer-use-overlay button")!.click());
    expect(container.querySelector(".computer-use-overlay.expanded figcaption")!.textContent).toContain("Recents");
    // A non-image URL is never shown.
    await act(async () => receivePreview({ itemId: "evt_2", taskId: "tsk", turnId: "trn_1", image: "https://example.com/x.png" }));
    expect(container.querySelector(".computer-use-overlay img")!.getAttribute("src")).toBe(image + "AA");
    // The turn ended (or another conversation is open): nothing floats.
    await act(async () => root.render(view(undefined)));
    expect(container.querySelector("img")).toBeNull();
    await act(async () => root.render(view("trn_2")));
    expect(container.querySelector("img")).toBeNull();
    clearPreviewsForTest();
  } finally { await act(async () => root.unmount()); }
});
