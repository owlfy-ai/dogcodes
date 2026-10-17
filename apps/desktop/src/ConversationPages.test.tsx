// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { ConversationPages } from "./ConversationPages";
it("restores the same DOM and disclosure state without rerendering cached history", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  const host = document.createElement("div"), root = createRoot(host);
  const render = vi.fn();
  function History({ name }: { name: string }) { render(name); return <details><summary>{name}</summary>Content</details>; }
  const a = {}, b = {};
  try {
    await act(async () => root.render(<ConversationPages taskId="a" revision={a}><History name="a" /></ConversationPages>));
    const details = host.querySelector("details")!;
    details.open = true;
    await act(async () => root.render(<ConversationPages taskId="b" revision={b}><History name="b" /></ConversationPages>));
    await act(async () => root.render(<ConversationPages taskId="a" revision={a}><History name="a" /></ConversationPages>));
    expect(host.querySelector("details")).toBe(details);
    expect(details.open).toBe(true);
    expect(render.mock.calls).toEqual([["a"], ["b"]]);
    await act(async () => root.render(<ConversationPages taskId="a" revision={[]}><History name="updated" /></ConversationPages>));
    expect(details.textContent).toContain("updated");
  } finally { await act(async () => root.unmount()); vi.unstubAllGlobals(); }
});

it("resumes bottom-follow for a new prompt while preserving manual reading and other cached pages", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  const resizes: (() => void)[] = [];
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { resizes.push(callback); }
    observe() {} disconnect() {}
  });
  let height = 1400;
  const heights = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const sizes = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(500);
  const host = document.createElement("div"), root = createRoot(host);
  const render = async (taskId: string, messageId: string) => act(async () => {
    root.render(<ConversationPages taskId={taskId} revision={{}} latestUserMessageId={messageId}>
      <p>{messageId}</p>
    </ConversationPages>);
  });
  const readOlder = async (node: HTMLElement) => act(async () => {
    node.dispatchEvent(new WheelEvent("wheel", { deltaY: -400, bubbles: true }));
    node.scrollTop = 300;
    node.dispatchEvent(new Event("scroll"));
  });
  try {
    await render("a", "prompt-1");
    const node = host.querySelector<HTMLElement>(".timeline")!;
    await readOlder(node);
    height = 1600;
    await render("a", "prompt-1"); // Refresh/assistant events must not force a jump.
    await act(async () => resizes[0]());
    expect(node.scrollTop).toBe(300);
    expect(host.textContent).toContain("回到底部");

    height = 1900;
    await render("a", "prompt-2");
    expect(node.scrollTop).toBe(1900);
    expect(host.textContent).not.toContain("回到底部");
    height = 2200;
    await act(async () => resizes[0]()); // Follow the response after sending.
    expect(node.scrollTop).toBe(2200);

    await readOlder(node);
    height = 2400;
    await render("a", "prompt-2");
    await act(async () => resizes[0]());
    expect(node.scrollTop).toBe(300);

    await render("b", "other-prompt");
    await render("b", "another-prompt");
    await render("a", "prompt-2");
    expect(host.querySelector(".timeline")).toBe(node);
    expect(node.scrollTop).toBe(300); // Another task's prompt must not reset A.
    height = 2700;
    await render("a", "prompt-3");
    expect(node.scrollTop).toBe(2700);
  } finally {
    await act(async () => root.unmount());
    heights.mockRestore(); sizes.mockRestore(); vi.unstubAllGlobals();
  }
});
