import {expect,it,vi} from "vitest";
import {ModelCatalogCache} from "./modelCatalogCache";
import type {ExecutorRef} from "@dogcodes/protocol";
const executor=(id:string)=>({providerId:id,installationId:`ins_${id}`} as ExecutorRef);
const catalog={models:[],defaultModel:null};
it("shares pending and completed reads, invalidates on version or explicit refresh",async()=>{
 const read=vi.fn(async()=>catalog);const cache=new ModelCatalogCache(read);
 const first=cache.get(executor("cursor"),"1");expect(cache.get(executor("cursor"),"1")).toBe(first);await first;
 await cache.get(executor("cursor"),"1");expect(read).toHaveBeenCalledTimes(1);
 await cache.get(executor("cursor"),"2");await cache.get(executor("cursor"),"2",true);expect(read).toHaveBeenCalledTimes(3);
});
it("bounds startup concurrency and continues after failure without automatic retries",async()=>{
 const finish:Array<()=>void>=[];let running=0,maximum=0;
 const read=vi.fn(async(ex:ExecutorRef)=>{running++;maximum=Math.max(maximum,running);await new Promise<void>(r=>finish.push(r));running--;if(ex.providerId==="bad")throw Error("offline");return catalog;});
 const cache=new ModelCatalogCache(read, []);
 const bad=cache.get(executor("bad"),null);void bad.catch(()=>{});
 const second=cache.get(executor("b"),null),third=cache.get(executor("c"),null);
 await Promise.resolve();expect(read).toHaveBeenCalledTimes(2);
 finish.shift()!();await expect(bad).rejects.toThrow("offline");await new Promise(r=>setTimeout(r,0));
 expect(read).toHaveBeenCalledTimes(3);expect(cache.get(executor("bad"),null)).toBe(bad);
 finish.splice(0).forEach(r=>r());await Promise.all([second,third]);expect(maximum).toBe(2);
});

it("automatically recovers a startup failure for an already waiting picker",async()=>{
 vi.useFakeTimers();
 try {
  const read=vi.fn().mockRejectedValueOnce(Error("starting")).mockResolvedValue(catalog);
  const cache=new ModelCatalogCache(read);
  const preload=cache.get(executor("cursor"),"1");
  const picker=cache.get(executor("cursor"),"1");
  expect(picker).toBe(preload);
  await vi.advanceTimersByTimeAsync(1500);
  await expect(picker).resolves.toEqual(catalog);
  expect(cache.peek(executor("cursor"),"1")).toEqual(catalog);
  expect(read).toHaveBeenCalledTimes(2);
 } finally {vi.useRealTimers();}
});
it("limits retries when an Agent remains unavailable",async()=>{
 vi.useFakeTimers();
 try {
  const read=vi.fn().mockRejectedValue(Error("offline"));
  const cache=new ModelCatalogCache(read);
  const pending=cache.get(executor("kimi"),"1");
  const rejected=expect(pending).rejects.toThrow("offline");
  await vi.runAllTimersAsync();await rejected;
  expect(read).toHaveBeenCalledTimes(3);
  expect(cache.get(executor("kimi"),"1")).toBe(pending);
 } finally {vi.useRealTimers();}
});

it("expires successful reads and coalesces manual refreshes while retaining stale data", async () => {
 let now=0;
 let finish!: (value:typeof catalog)=>void;
 const read=vi.fn().mockResolvedValueOnce(catalog).mockImplementation(()=>new Promise(r=>{finish=r;}));
 const cache=new ModelCatalogCache(read,[],()=>now);
 await cache.get(executor("a"),"1");
 now=300001;
 expect(cache.peek(executor("a"),"1")).toBeUndefined();
 expect(cache.peek(executor("a"),"1",{},true)).toEqual(catalog);
 const pending=cache.get(executor("a"),"1");
 expect(cache.get(executor("a"),"1",true)).toBe(pending);
 await Promise.resolve();finish(catalog);await pending;
 expect(read).toHaveBeenCalledTimes(2);
});
it("isolates model efforts and workspaces and allows recovery after a cached failure expires",async()=>{
 let now=0; const read=vi.fn().mockResolvedValue(catalog);
 const cache=new ModelCatalogCache(read,[],()=>now);
 for (const scope of [{model:"a",taskId:"tsk_one"},{model:"b",taskId:"tsk_one"},{model:"a",taskId:"tsk_two"}] as const) await cache.get(executor("pi"),"1",false,scope);
 expect(read).toHaveBeenCalledTimes(3);
 expect(read).toHaveBeenLastCalledWith(executor("pi"),"a","tsk_two");
 read.mockRejectedValueOnce(Error("offline"));
 await expect(cache.get(executor("pi"),"1",true,{model:"a",taskId:"tsk_two"})).rejects.toThrow("offline");
 expect(cache.peek(executor("pi"),"1",{model:"a",taskId:"tsk_two"},true)).toEqual(catalog);
 now=30001;
 await cache.get(executor("pi"),"1",false,{model:"a",taskId:"tsk_two"});
 expect(read).toHaveBeenCalledTimes(5);
});
