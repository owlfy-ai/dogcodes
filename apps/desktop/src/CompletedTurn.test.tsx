// @vitest-environment jsdom
import {act} from "react";
import {createRoot} from "react-dom/client";
import {afterEach, beforeEach, expect, it, vi} from "vitest";
import type {CoreClient, TimelineItem} from "@dogcodes/protocol";
import {ConversationContent} from "./ConversationActivity";
import {TurnFiles} from "./TurnFiles";
vi.mock("./TurnFiles",()=>({TurnFiles:vi.fn(()=> <div data-testid="turn-files">文件变更摘要</div>)}));
const item=(id:string,kind:string,text:string):TimelineItem=>({attachments: [], itemId:id,taskId:"tsk_test",turnId:"trn_one",kind,text,providerId:"codex",sequence:1,occurredAt:"2026-09-15T10:00:00Z"});
const activity=(id:string,data:object)=>item(id,"activity",JSON.stringify(data));
const completion={startedAt:"2026-09-15T10:00:00Z",completedAt:"2026-09-15T10:03:21Z"};
let host:HTMLDivElement,root:ReturnType<typeof createRoot>;
beforeEach(()=>{vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true);host=document.createElement("div");document.body.append(host);root=createRoot(host);});
afterEach(async()=>{await act(async()=>root.unmount());host.remove();vi.unstubAllGlobals();});
const render=async(items:TimelineItem[],activeTurnId?:string)=>act(async()=>root.render(<ConversationContent items={items} client={{} as CoreClient} activeTurnId={activeTurnId} onSubagent={()=>{}} renderAgent={provider=><span data-provider={provider} aria-label={provider}/>} renderEntry={i=><p key={i.itemId} data-kind={i.kind}>{i.text}</p>}/>));
const history=()=>[
  {...item("prompt","user_message","用户提示词"),turnCompletion:completion},
  activity("thought",{type:"thought",status:"completed",durationMs:2000}),
  item("commentary","assistant_message","我先检查文件。"),
  activity("read",{type:"tool",name:"Read",status:"completed",input:{file_path:"a.ts"}}),
  activity("read2",{type:"tool",name:"Read",status:"completed",input:{file_path:"b.ts"}}),
  item("files","turn_files","{}"),
  item("final","assistant_message","最终回答"),
];
it.each([false,true])("collapses work before the final answer and keeps prompt/files outside (native activity=%s)",async(native)=>{
  const items=history();
  if(native)items[items.length-1]=activity("final",{type:"message",text:"最终回答"});
  await render(items);
  const details=host.querySelector<HTMLDetailsElement>(".turn-process")!;
  expect(details.open).toBe(false);
  expect(details.querySelector('.turn-process-heading > .turn-process-agent [data-provider="codex"]')).not.toBeNull();
  expect(host.querySelectorAll('[data-provider]')).toHaveLength(1);
  expect(details.querySelector(".turn-process-heading")!.textContent).toBe("已工作 3 分 21 秒");
  expect(details.querySelector(".turn-process-body")).toBeNull();
  expect(details.textContent).not.toContain("最终回答");
  expect(details.querySelector('[data-kind="user_message"]')).toBeNull();
  expect(details.querySelector('[data-testid="turn-files"]')).toBeNull();
  expect(host.querySelector('[data-testid="turn-files"]')).not.toBeNull();
  expect(host.lastElementChild!.querySelector('[aria-label="本轮操作"]')).not.toBeNull();
  await act(async()=>details.querySelector("summary")!.click());
  expect(details.open).toBe(true);
  await act(async()=>details.dispatchEvent(new Event("toggle")));
  expect(details.textContent).toContain("我先检查文件");
  expect(details.textContent).toContain("查阅");
  const body = details.querySelector(".turn-process-body");
  await act(async()=>{ details.open = false; details.dispatchEvent(new Event("toggle")); });
  expect(details.querySelector(".turn-process-body")).toBe(body);
  await act(async()=>{ details.open = true; details.dispatchEvent(new Event("toggle")); });
  await render([...items]);
  expect(host.querySelector(".turn-process")).toBe(details);
  expect(details.open).toBe(true);
});
it("keeps active, waiting, failed or unconfirmed turns expanded",async()=>{
  const items=history();
  await render(items,"trn_one");
  expect(host.querySelector(".turn-process")).toBeNull();
  await render(items.map(({turnCompletion:_,...i})=>i));
  expect(host.querySelector(".turn-process")).toBeNull();
});
it("does not mount historical artifact cards inside a collapsed process", async () => {
  vi.mocked(TurnFiles).mockClear();
  const items = history();
  items.splice(2, 0, ...Array.from({ length: 100 }, (_, index) => activity(`edit-${index}`, {
    type: "tool", name: "Edit", status: "completed", artifact: { artifactId: `art_${index}` },
  })));
  await render(items);
  // Only the visible turn summary loads; historical operations stay dormant.
  expect(TurnFiles).toHaveBeenCalledTimes(1);
  const details = host.querySelector<HTMLDetailsElement>(".turn-process")!;
  await act(async () => { details.open = true; details.dispatchEvent(new Event("toggle")); });
  expect(host.querySelectorAll('[data-testid="turn-files"]')).toHaveLength(101);
});
it("does not hide a completed turn without any final response",async()=>{
  await render(history().filter(i=>i.kind!=="assistant_message"));
  expect(host.querySelector(".turn-process")).toBeNull();
});
it("shows statistics for a terminal failure and hides them while a turn is active", async () => {
  const items = [history()[0], item("failure", "system_message", "执行已中断")];
  await render(items, "trn_one");
  expect(host.querySelector('[aria-label="本轮数据统计"]')).toBeNull();
  await render(items);
  expect(host.textContent).toContain("执行已中断");
  expect(host.querySelectorAll('[aria-label="本轮数据统计"]')).toHaveLength(1);
});
it("folds a turn only after completion arrives and leaves the next turn live",async()=>{
  const items=history();
  await render(items.map(({turnCompletion:_,...i})=>i),"trn_one");
  expect(host.querySelector(".turn-process")).toBeNull();
  const next=history().slice(0,4).map(({turnCompletion:_,...i})=>({...i,itemId:`next-${i.itemId}`,turnId:"trn_two" as const}));
  await render([...items,...next],"trn_two");
  expect(host.querySelectorAll(".turn-process")).toHaveLength(1);
  expect(host.querySelector(".turn-process")!.textContent).not.toContain("next-");
  expect(host.querySelectorAll('[data-kind="user_message"]')).toHaveLength(2);
  expect(host.lastElementChild!.closest(".turn-process")).toBeNull();
});
