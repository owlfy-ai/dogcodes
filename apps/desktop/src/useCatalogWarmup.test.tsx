// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import type { CoreClient, InstallationSummary } from "@dogcodes/protocol";
import { ModelCatalogCache } from "./modelCatalogCache";
import { permissionCatalogEntry } from "./permissionCatalogCache";
import { useCatalogWarmup } from "./useCatalogWarmup";
it("warms every runtime-ready Agent, keeps successful inventory, and recovers after Core revision changes",async()=>{
 vi.useFakeTimers();vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true);
 const host=document.createElement("div"),root=createRoot(host);let revision=1;
 const rows=["pi","kimi","unregistered"].map(providerId=>({providerId,installationId:`ins_${providerId}`,version:"1"})) as InstallationSummary[];
 const availableModels=vi.fn(async()=>({models:[],defaultModel:null}));
 const permissionCatalog=vi.fn(async()=>({modes:[],defaultModeId:null}));
 const client={catalogRevision:async()=>revision,permissionCatalog} as unknown as CoreClient;
 const cache=new ModelCatalogCache(availableModels,[]);
 function Probe(){useCatalogWarmup(client,cache,rows,["pi:ins_pi","kimi:ins_kimi"]);return null;}
 try{
  await act(async()=>root.render(<Probe/>));expect(availableModels).toHaveBeenCalledTimes(2);expect(permissionCatalog).toHaveBeenCalledTimes(2);
  const executor={providerId:"pi",installationId:"ins_pi"};expect(cache.peek(executor,"1")).toBeDefined();
  expect(permissionCatalogEntry(client,executor,undefined,"1").peek()).toBeDefined();
  await act(async()=>vi.advanceTimersByTimeAsync(360000));expect(availableModels).toHaveBeenCalledTimes(2);
  revision++;await act(async()=>vi.advanceTimersByTimeAsync(5000));expect(availableModels).toHaveBeenCalledTimes(4);expect(permissionCatalog).toHaveBeenCalledTimes(4);
 }finally{await act(async()=>root.unmount());vi.useRealTimers();vi.unstubAllGlobals();}
});
