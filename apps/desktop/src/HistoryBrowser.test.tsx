// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { CoreClient, HistoryPage, TaskId } from "@dogcodes/protocol";
import { HistoryBrowser } from "./HistoryBrowser";
let root: Root; let host: HTMLDivElement;
beforeEach(()=>{vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true);host=document.createElement("div");document.body.append(host);root=createRoot(host);});
afterEach(async()=>{await act(async()=>root.unmount());host.remove();vi.unstubAllGlobals();});
const page=(text:string,cursor:number|null=null):HistoryPage=>({items:[{sequence:cursor??2,turnId:"turn",kind:"user",text,occurredAt:"2026-09-11T12:00:00Z"}],nextCursor:cursor,throughSequence:42,coverage:"fixture"});
it("freezes pagination upper bound and ignores late responses after switching tasks",async()=>{
 let resolveOld!:(p:HistoryPage)=>void;
 const queryHistory=vi.fn().mockResolvedValueOnce(page("first",1)).mockImplementationOnce(()=>new Promise<HistoryPage>(resolve=>{resolveOld=resolve;})).mockResolvedValueOnce(page("new task"));
 const client={queryHistory} as unknown as CoreClient;
 await act(async()=>root.render(<HistoryBrowser client={client} taskId={"task-a" as TaskId} onBack={()=>{}}/>));
 await act(async()=>[...host.querySelectorAll("button")].find(b=>b.textContent==="加载下一页")!.click());
 expect(queryHistory.mock.calls[1][0]).toMatchObject({afterSequence:1,throughSequence:42});
 await act(async()=>root.render(<HistoryBrowser client={client} taskId={"task-b" as TaskId} onBack={()=>{}}/>));
 await act(async()=>resolveOld(page("stale")));
 expect(host.textContent).toContain("new task");expect(host.textContent).not.toContain("stale");expect(host.textContent).not.toContain("first");
});
it("permits continuing empty canonical scan pages",async()=>{
 const queryHistory=vi.fn().mockResolvedValue({...page(""),items:[],nextCursor:1000});
 await act(async()=>root.render(<HistoryBrowser client={{queryHistory} as unknown as CoreClient} taskId={"task-a" as TaskId} onBack={()=>{}}/>));
 expect(host.textContent).toContain("可继续查找");
});

it("offers a return action for empty history and clears an unsuccessful search", async () => {
  const queryHistory = vi.fn().mockResolvedValue({ ...page(""), items: [] });
  const onBack = vi.fn();
  await act(async () => root.render(<HistoryBrowser client={{ queryHistory } as unknown as CoreClient} taskId={"task-a" as TaskId} onBack={onBack} />));
  expect(host.querySelector(".review-empty-state h3")?.textContent).toBe("暂无对话记录");
  await act(async () => host.querySelector<HTMLButtonElement>(".review-empty-action")!.click());
  expect(onBack).toHaveBeenCalledTimes(1);
  const input = host.querySelector("input")!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, "missing");
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => host.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true })));
  expect(host.querySelector(".review-empty-state h3")?.textContent).toBe("没有找到相关内容");
  await act(async () => host.querySelector<HTMLButtonElement>(".review-empty-action")!.click());
  expect(input.value).toBe(""); expect(document.activeElement).toBe(input);
  expect(queryHistory).toHaveBeenLastCalledWith(expect.objectContaining({ text: "", kind: "turns" }));
});

it("continues an empty scan using the frozen cursor instead of treating it as final", async () => {
  const queryHistory = vi.fn().mockResolvedValueOnce({ ...page(""), items: [], nextCursor: 1000 }).mockResolvedValueOnce(page("找到记录"));
  await act(async () => root.render(<HistoryBrowser client={{ queryHistory } as unknown as CoreClient} taskId={"task-a" as TaskId} onBack={() => {}} />));
  expect(host.querySelector(".review-empty-state h3")?.textContent).toBe("本段没有匹配记录");
  await act(async () => [...host.querySelectorAll("button")].find(button => button.textContent === "继续查找")!.click());
  expect(queryHistory).toHaveBeenLastCalledWith(expect.objectContaining({ afterSequence: 1000, throughSequence: 42 }));
  expect(host.querySelector(".review-empty-state")).toBeNull();
  expect(host.textContent).toContain("找到记录");
});
