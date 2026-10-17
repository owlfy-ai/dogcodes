// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ModelPicker } from "./ModelPicker";
import type { ModelCatalog } from "@dogcodes/protocol";
let root: Root;
let container: HTMLDivElement;
const change = vi.fn();
const retry = vi.fn();
const catalog: ModelCatalog = { defaultModel: "grok", models: [
  {id:"grok",label:"Grok 4.6",thinkingOptions:[{id:"low",label:"Low Effort"},{id:"high",label:"High Effort"}],defaultThinking:"high"},
  {id:"plain",label:"Plain",thinkingOptions:[],defaultThinking:null},
] };
beforeEach(() => { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT",true); change.mockReset(); retry.mockReset(); container=document.createElement("div"); document.body.appendChild(container); root=createRoot(container); });
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
const click = async (node: Element) => { await act(async () => node.dispatchEvent(new MouseEvent("click",{bubbles:true}))); };
const button = (name:string) => container.querySelector(`[aria-label="${name}"]`)!;
async function render(model: string|null="grok", error: string|null=null) {
  await act(async () => root.render(<ModelPicker catalog={catalog} model={model} thinking={null} loading={false} error={error} disabled={false} onChange={change} onRetry={retry}/>));
}
it("keeps model and thinking independent and drops efforts when choosing a model without them",async () => {
  await render(); await click(button("选择思考强度"));
  expect(container.querySelector('[role="dialog"]')?.getAttribute("aria-label")).toBe("思考强度");
  await click(container.querySelector('.model-options button')!);
  expect(change).toHaveBeenLastCalledWith("grok","low");
  await click(button("选择模型"));
  expect(container.querySelector("input")).toBeNull();
  await click([...container.querySelectorAll('.model-options button')].find(node=>node.textContent==="Plain")!);
  expect(change).toHaveBeenLastCalledWith("plain",null);
  await render("plain"); expect(container.querySelector('[aria-label="选择思考强度"]')).toBeNull();
});
it("lists models directly without a search box and closes with Escape",async () => {
  await render(); await click(button("选择模型"));
  expect(container.querySelector("input")).toBeNull();
  expect(container.querySelector('.model-options')?.textContent).toContain("Grok 4.6");
  expect(container.querySelector('.model-options')?.textContent).toContain("Plain");
  await act(async()=>document.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true})));
  expect(container.querySelector('[role="dialog"]')).toBeNull(); expect(change).not.toHaveBeenCalled();
});
it("retries on opening an unavailable catalog and keeps only the header refresh icon",async () => {
  await render(null,"failed"); await click(button("选择模型"));
  expect(container.querySelector('.model-menu')?.textContent).not.toContain("重新尝试");
  expect(container.querySelector('.model-message')?.textContent).toContain("刷新失败");
  expect(button("刷新模型列表").textContent).toBe("");
  expect(button("刷新模型列表").querySelector('svg[aria-hidden="true"]')).not.toBeNull();
  await click(button("刷新模型列表"));
  expect(retry).toHaveBeenCalledTimes(2); expect(change).not.toHaveBeenCalled();
});
it("refreshes either list without changing selection and keeps old options after failure",async()=>{
 await render();await click(button("选择模型"));await click(button("刷新模型列表"));
 expect(retry).toHaveBeenCalledTimes(1);expect(change).not.toHaveBeenCalled();
 await click(button("选择思考强度"));await click(button("刷新思考强度列表"));
 expect(button("刷新思考强度列表").textContent).toBe("");
 expect(button("刷新思考强度列表").querySelector('svg[aria-hidden="true"]')).not.toBeNull();
 expect(retry).toHaveBeenCalledTimes(2);
 await render("grok","offline");
 expect(container.textContent).toContain("保留上次列表");
 expect(container.querySelector('.model-options')?.textContent).toContain("低");
 expect(change).not.toHaveBeenCalled();
});
it("preserves removed model and thinking choices and disables refresh while busy",async()=>{
 await act(async()=>root.render(<ModelPicker catalog={catalog} model="grok" thinking="retired" loading={false} error={null} disabled={false} refreshDisabled onChange={change} onRetry={retry}/>));
 await click(button("选择思考强度"));
 expect(container.textContent).toContain("已选思考强度不在当前列表");
 expect((button("刷新思考强度列表") as HTMLButtonElement).disabled).toBe(true);
 expect(change).not.toHaveBeenCalled();
 await render("removed"); await click(button("选择模型"));
 expect(container.textContent).toContain("已选模型不在当前列表");
 expect(change).not.toHaveBeenCalled();
});
it("requires an explicit available model when the native default is retired",async()=>{
 const unavailable = {...catalog, defaultModel:"opencode/retired"};
 await act(async()=>root.render(<ModelPicker catalog={unavailable} model={null} thinking={null} loading={false} error={null} disabled={false} onChange={change} onRetry={retry}/>));
 expect(container.textContent).toContain("请选择模型");
 await click(button("选择模型"));
 expect(container.textContent).toContain("默认模型已不可用，请选择其他模型");
 const nativeDefault = [...container.querySelectorAll('button')].find(node=>node.textContent==="默认模型不可用")!;
 expect((nativeDefault as HTMLButtonElement).disabled).toBe(true);
 expect(container.textContent).not.toContain("默认（推荐）");
 await click(container.querySelector('.model-options button')!);
 expect(change).toHaveBeenLastCalledWith("grok","high");
});
