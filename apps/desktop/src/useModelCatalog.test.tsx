// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { ModelCatalog } from "@dogcodes/protocol";
import { ModelCatalogCache } from "./modelCatalogCache";
import { useModelCatalog } from "./useModelCatalog";
let root: Root, el: HTMLDivElement;
let result: ReturnType<typeof useModelCatalog>;
const executor={providerId:"pi",installationId:"ins_pi"} as const;
const catalog=(id:string):ModelCatalog=>({defaultModel:id,models:[{id,label:id,thinkingOptions:[],defaultThinking:null}]});
function Harness({cache,model="a",busy=false}:{cache:ModelCatalogCache;model?:string;busy?:boolean}) {
 result=useModelCatalog(cache,executor,"1","tsk_one",model,true,busy);
 return null;
}
beforeEach(()=>{vi.useFakeTimers();vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true);el=document.createElement("div");document.body.append(el);root=createRoot(el);});
afterEach(async()=>{await act(async()=>root.unmount());el.remove();vi.useRealTimers();vi.unstubAllGlobals();vi.restoreAllMocks();});
it("checks TTL on focus/menu opening and defers automatic refresh until visible and idle",async()=>{
 const read=vi.fn(async()=>catalog("a"));const cache=new ModelCatalogCache(read,[]);
 await act(async()=>root.render(<Harness cache={cache}/>));expect(read).toHaveBeenCalledTimes(1);
 await act(async()=>{result.check();window.dispatchEvent(new Event("focus"));});expect(read).toHaveBeenCalledTimes(1);
 await act(async()=>root.render(<Harness cache={cache} busy/>));
 await act(async()=>vi.advanceTimersByTimeAsync(360000));expect(read).toHaveBeenCalledTimes(1);
 expect(result.catalog.models[0].id).toBe("a");
 const visibility=vi.spyOn(document,"visibilityState","get").mockReturnValue("hidden");
 await act(async()=>root.render(<Harness cache={cache}/>));expect(read).toHaveBeenCalledTimes(1);
 visibility.mockReturnValue("visible");
 await act(async()=>document.dispatchEvent(new Event("visibilitychange")));expect(read).toHaveBeenCalledTimes(2);
});
it("ignores late model responses and refreshes current-model efforts without changing selection",async()=>{
 const resolve=new Map<string,(c:ModelCatalog)=>void>();
 const read=vi.fn((_e,m:string|null)=>new Promise<ModelCatalog>(r=>resolve.set(m!,r)));
 const cache=new ModelCatalogCache(read,[]);
 await act(async()=>root.render(<Harness cache={cache}/>));
 await act(async()=>root.render(<Harness cache={cache} model="b"/>));
 await act(async()=>resolve.get("b")!(catalog("b")));
 await act(async()=>resolve.get("a")!(catalog("a")));
 expect(result.catalog.defaultModel).toBe("b");
 await act(async()=>result.refresh());
 expect(result.catalog.defaultModel).toBe("b");expect(result.loading).toBe(true);
 await act(async()=>window.dispatchEvent(new Event("focus")));
 expect(read).toHaveBeenCalledTimes(3);
 expect(read).toHaveBeenLastCalledWith(executor,"b","tsk_one");
 await act(async()=>resolve.get("b")!(catalog("b-new")));
 expect(result.catalog.defaultModel).toBe("b-new");
});
